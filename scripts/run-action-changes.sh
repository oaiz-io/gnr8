#!/usr/bin/env bash
# Run one API change report per project while preserving exit 1 for optional final enforcement.
set -euo pipefail

: "${GNR8_BIN:?GNR8_BIN is required}"
: "${BASE_REF:?BASE_REF is required}"
: "${WORKING_DIRECTORIES:?WORKING_DIRECTORIES is required}"
: "${RUNNER_TEMP:?RUNNER_TEMP is required}"
: "${GITHUB_OUTPUT:?GITHUB_OUTPUT is required}"
: "${GITHUB_STEP_SUMMARY:?GITHUB_STEP_SUMMARY is required}"

fail_on_breaking="${FAIL_ON_BREAKING:-true}"

# The per-project heading is the only report text this script writes; everything under it comes from
# `gnr8 changes --markdown`, which is the one renderer for that format. The directory lands in a
# Markdown document outside a code block, so it is escaped the way the CLI escapes what it puts there.
escape_html() {
  local flat="$1"
  flat="${flat//$'\r'/ }"
  flat="${flat//$'\n'/ }"
  printf '%s' "$flat" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g' \
    -e 's/"/\&quot;/g' -e "s/'/\&#x27;/g"
}

# GitHub output values can contain newlines (for example, a runner temp path). Delimit those
# values by their own content hash rather than exposing embedded lines as additional outputs.
write_output() {
  local name="$1" value="$2" delimiter
  if [[ "$value" == *$'\n'* ]]; then
    delimiter="gnr8_$(printf '%s' "$value" | git hash-object --stdin)"
    printf '%s<<%s\n%s\n%s\n' "$name" "$delimiter" "$value" "$delimiter"
  else
    printf '%s=%s\n' "$name" "$value"
  fi
}

summary_failed() {
  echo "::warning::gnr8 action: could not publish the step summary; full reports are in the \"$artifact_name\" artifact. The API change gate is unchanged."
  summary_stopped=true
}

# Run one report for $dir into $1 and hand back the gate status. Only 0 and 1 are gate answers; any
# other status is a failed analysis and stops the whole step.
report() {
  local destination="$1"
  shift
  local status=0
  (cd "$dir" && "$GNR8_BIN" "$@") > "$destination" 2> "$stderr" || status=$?
  if [[ -s "$stderr" ]]; then
    cat "$stderr" >&2
  fi
  if [[ "$status" -ne 0 && "$status" -ne 1 ]]; then
    echo "gnr8 action: change analysis failed for '$dir' with status $status" >&2
    exit "$status"
  fi
  return "$status"
}

report_root="$(mktemp -d "$RUNNER_TEMP/gnr8-api-changes.XXXXXXXX")"
# Intermediates live outside report_root, which is uploaded verbatim as the run's artifact.
work_root="$(mktemp -d "$RUNNER_TEMP/gnr8-api-changes-work.XXXXXXXX")"
artifact_suffix="$(printf '%s\n%s\n' "$WORKING_DIRECTORIES" "$BASE_REF" | git hash-object --stdin | cut -c1-8)"
artifact_name="gnr8-api-changes-${GITHUB_JOB:-job}-$artifact_suffix"
marker="<!-- gnr8-api-changes:${artifact_name} -->"
combined="$report_root/report.md"
printf '# gnr8 API changes\n\n' > "$combined"
# These outputs describe the publication destination even when a later project cannot be analyzed.
{
  write_output report-root "$report_root"
  write_output artifact-name "$artifact_name"
  write_output marker "$marker"
} >> "$GITHUB_OUTPUT"

# Budget whole project blocks, never cut the CLI's Markdown or re-render findings.
summary_budget=$((900 * 1024))
summary_stopped=false
summary_bytes=0
if ! cat "$combined" >> "$GITHUB_STEP_SUMMARY" || ! summary_bytes="$(wc -c < "$GITHUB_STEP_SUMMARY")"; then
  summary_failed
fi

dirs=()
while IFS= read -r dir || [[ -n "$dir" ]]; do
  dir="${dir#"${dir%%[![:space:]]*}"}"
  dir="${dir%"${dir##*[![:space:]]}"}"
  [[ -z "$dir" || "$dir" == \#* ]] && continue
  dirs+=("$dir")
done <<< "$WORKING_DIRECTORIES"

if [[ "${#dirs[@]}" -eq 0 ]]; then
  echo "gnr8 action: no working directories configured" >&2
  exit 2
fi

# Seeded with the argv both reports share so the array is never empty: expanding "${empty[@]}" under
# `set -u` is an unbound-variable error in bash 3.2, the bash GitHub's macOS runners provide, and the
# default configuration (no exempt-tags) appends nothing to it.
change_args=(changes --base "$BASE_REF")
if [[ -n "${ACCEPTANCE_FILE:-}" ]]; then
  # Preserve the configured path byte-for-byte and let the CLI resolve it from each project root.
  change_args+=(--acceptance-file "$ACCEPTANCE_FILE")
fi
while IFS= read -r tag || [[ -n "$tag" ]]; do
  # Tag matching is exact. Empty lines separate values; every byte on a non-empty line belongs to
  # the OpenAPI tag, including leading/trailing spaces and a leading '#'. The CLI performs the one
  # canonical validity check (blank-only and multiline values are invalid).
  [[ -z "$tag" ]] && continue
  change_args+=(--exempt-tag "$tag")
done <<< "${EXEMPT_TAGS:-}"
while IFS= read -r operation || [[ -n "$operation" ]]; do
  # The CLI parses and validates the one canonical `METHOD /path` form. Keep every non-empty line
  # byte-for-byte so malformed or unmatched selectors become actionable status-2 failures.
  [[ -z "$operation" ]] && continue
  change_args+=(--gate-operation "$operation")
done <<< "${GATE_OPERATIONS:-}"

annotate="${ANNOTATE_API_CHANGES:-true}"
if [[ "$annotate" == true ]] && ! command -v python3 >/dev/null 2>&1; then
  echo 'gnr8 action: annotate-api-changes requires python3; install python3 or set annotate-api-changes to "false"' >&2
  exit 2
fi
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

gating=false
breaking_changes=false
breaking_count=0
gating_count=0
metadata_complete=true
index=0
for dir in "${dirs[@]}"; do
  if ! (cd "$dir" && git rev-parse --verify --quiet --end-of-options "${BASE_REF}^{commit}" >/dev/null); then
    echo "gnr8 action: base ref '$BASE_REF' is unavailable from '$dir'; checkout with fetch-depth: 0 and ensure the ref exists" >&2
    exit 2
  fi

  index=$((index + 1))
  project_root="$report_root/$(printf '%03d' "$index")"
  mkdir -p "$project_root"
  json="$project_root/report.json"
  markdown="$project_root/report.md"
  body="$work_root/$(printf '%03d' "$index").md"
  stderr="$project_root/stderr.txt"

  echo "::group::gnr8 changes project $index"
  # Each format is rendered by the CLI. The Markdown report is a second invocation rather than a
  # second implementation of the format here: `changes` is deterministic and this run reads the
  # cache the first one just filled, so it costs a fraction of it and no formatting lives here.
  status=0
  report "$json" --json "${change_args[@]}" || status=$?
  body_status=0
  report "$body" "${change_args[@]}" --markdown || body_status=$?
  if [[ "$status" -ne "$body_status" ]]; then
    echo "gnr8 action: '$dir' reported gate status $status as JSON and $body_status as Markdown" >&2
    exit 2
  fi
  if [[ "$status" -eq 1 ]]; then
    gating=true
  fi

  project_breaking="$(sed -n 's/^[[:space:]]*"breaking": \([0-9][0-9]*\),$/\1/p' "$json")"
  project_gating="$(sed -n 's/^[[:space:]]*"gating": \([0-9][0-9]*\)$/\1/p' "$json")"
  if [[ "$project_breaking" =~ ^[0-9]+$ && "$project_gating" =~ ^[0-9]+$ ]]; then
    breaking_count=$((breaking_count + project_breaking))
    gating_count=$((gating_count + project_gating))
    if [[ "$project_breaking" -gt 0 ]]; then
      breaking_changes=true
    fi
  else
    metadata_complete=false
    echo "::warning::gnr8 action: could not read summary counts from '$dir/report.json'; reports are still published and the API change gate is unchanged."
  fi

  {
    printf '%s\n' "$marker"
    printf '## API changes for %s\n\n' "$(escape_html "$dir")"
    if [[ "$fail_on_breaking" == true ]]; then
      printf 'Enforcement: protected-surface breaking changes fail this Action.\n\n'
    else
      printf 'Enforcement: advisory — breaking changes are reported without failing this Action.\n\n'
    fi
    cat "$body"
  } > "$markdown"

  cat "$markdown" >> "$combined"
  if [[ "$summary_stopped" == false ]]; then
    project_bytes="$(wc -c < "$markdown")"
    if [[ $((summary_bytes + project_bytes)) -le "$summary_budget" ]]; then
      if cat "$markdown" >> "$GITHUB_STEP_SUMMARY"; then
        summary_bytes=$((summary_bytes + project_bytes))
      else
        summary_failed
      fi
    else
      if ! {
        printf '\nReport truncated at 900 KiB (GitHub limits a step summary to 1 MiB).\n'
        printf 'Full Markdown and JSON: the "%s" artifact.\n' "$artifact_name"
      } >> "$GITHUB_STEP_SUMMARY"; then
        summary_failed
      fi
      summary_stopped=true
    fi
  fi
  if [[ "$annotate" == true ]]; then
    annotation_status=0
    if [[ "$fail_on_breaking" == true ]]; then
      python3 "$script_dir/emit-action-annotations.py" "$json" "$dir" "$artifact_name" \
        || annotation_status=$?
    else
      python3 "$script_dir/emit-action-annotations.py" --advisory \
        "$json" "$dir" "$artifact_name" || annotation_status=$?
    fi
    if [[ "$annotation_status" -ne 0 ]]; then
      echo "::warning::gnr8 action: could not publish API change annotations; see the reports in the \"$artifact_name\" artifact. The API change gate is unchanged."
    fi
  fi
  echo "::endgroup::"
done

{
  write_output gating "$gating"
  if [[ "$metadata_complete" == true ]]; then
    write_output breaking-changes "$breaking_changes"
    write_output breaking-count "$breaking_count"
    write_output gating-count "$gating_count"
  fi
  write_output combined-report "$combined"
  write_output report-digest "$(git hash-object "$combined")"
} >> "$GITHUB_OUTPUT"

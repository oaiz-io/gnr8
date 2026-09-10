#!/usr/bin/env bash
set -euo pipefail

# `! grep` alone is exempt from errexit, so negative assertions must fail explicitly.
assert_absent() {
  local status=0
  grep "$@" || status=$?
  if [[ "$status" -ne 1 ]]; then
    echo "expected no matching output (grep status $status)" >&2
    exit 1
  fi
}

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
runner="$repo_root/scripts/run-action-changes.sh"
tmp="$(mktemp -d)"
# A working directory whose name is meaningful in Markdown, inside the checkout so the runner's base
# ref resolves from it. `target/` is ignored, so this leaves the tree clean.
weird_dir="$repo_root/target/gnr8-action-changes-test/a<b>&c"
mkdir -p "$weird_dir"
trap 'rm -rf "$tmp" "$repo_root/target/gnr8-action-changes-test"' EXIT

# The fake renders nothing: it returns what the real CLI returns for each format, so the runner is
# tested against the CLI's contract rather than against a second implementation of the report.
fake="$tmp/gnr8"
cat > "$fake" <<'SH'
#!/usr/bin/env bash
printf '%q ' "$@" >> "$FAKE_LOG"
printf '\n' >> "$FAKE_LOG"
if [[ -n "${FAIL_PROJECT:-}" && "$PWD" == "$FAIL_PROJECT" ]]; then exit 2; fi
if [[ "${INVALID_JSON:-false}" == true && "$1" == --json ]]; then
  printf '{}\n'
  exit 1
fi
for arg in "$@"; do
  if [[ "$arg" == "--markdown" ]]; then
    if [[ "${OVERSIZED:-false}" == true ]]; then
      python3 -c 'print("    " + "x" * (901 * 1024))'
      exit 1
    fi
    cat <<'MARKDOWN'
Base: <code>HEAD</code> → <code>0123456789012345678901234567890123456789</code>

Exempt tags: <code>internal</code>

Protected operations: <code>POST /books</code>

Acceptance list: <code>reviewed-api-changes.json</code>

Summary: 1 breaking changes detected; 0 accepted after review; 1 protected-surface breaking changes; 0 additive changes; 0 documentation-only changes.

Breaking — protected surface (1)

    BREAKING  DELETE /books/{id}  operation removed ## injected heading
        Code: operation.removed
        SDK operations: deleteBook (DELETE /books/{id}), listBooks (GET /books)
        Source: handlers/<books>.go:42
MARKDOWN
    exit 1
  fi
done
cat <<'JSON'
{
  "schema_version": 1,
  "base": {"ref": "HEAD", "resolved": "0123456789012345678901234567890123456789"},
  "policy": {
    "exempt_tags": ["internal"],
    "gate_operations": ["POST /books"],
    "acceptance_file": "reviewed-api-changes.json"
  },
  "summary": {
    "breaking": 1,
    "additive": 0,
    "doc_only": 0,
    "accepted": 0,
    "gating": 1
  },
  "changes": [{
    "kind": "breaking",
    "code": "operation.removed",
    "operation": "DELETE /books/{id}",
    "affected_operations": {
      "base": [
        {"operation": "DELETE /books/{id}", "operation_id": "deleteBook"},
        {"operation": "GET /books", "operation_id": "listBooks"}
      ],
      "current": null
    },
    "tags": {"base": ["books"], "current": null},
    "exempt": {"base": false, "current": null},
    "protected": {"base": true, "current": null},
    "gating": true,
    "message": "operation removed\n## injected heading",
    "file": "handlers/<books>.go",
    "line": 42
  }]
}
JSON
exit 1
SH
chmod +x "$fake"

output="$tmp/output"
summary="$tmp/summary"
log="$tmp/args"
: > "$output"
: > "$summary"
: > "$log"

GNR8_BIN="$fake" \
BASE_REF=HEAD \
ACCEPTANCE_FILE=reviewed-api-changes.json \
EXEMPT_TAGS=$'internal\ninternal\n#partner\n partner APIs ' \
GATE_OPERATIONS=$'POST /books\nGET /reports' \
WORKING_DIRECTORIES="$repo_root/examples/bookstore" \
RUNNER_TEMP="$tmp" \
GITHUB_OUTPUT="$output" \
GITHUB_STEP_SUMMARY="$summary" \
GITHUB_JOB=test \
FAKE_LOG="$log" \
  "$runner" > "$tmp/run-stdout"

grep -Fx 'gating=true' "$output" >/dev/null
grep -Fx 'breaking-changes=true' "$output" >/dev/null
grep -Fx 'breaking-count=1' "$output" >/dev/null
grep -Fx 'gating-count=1' "$output" >/dev/null
grep -Ex 'report-digest=[0-9a-f]+' "$output" >/dev/null
grep -F 'artifact-name=gnr8-api-changes-test-' "$output" >/dev/null
grep -F 'BREAKING  DELETE /books/{id}  operation removed ## injected heading' "$summary" >/dev/null
grep -F 'SDK operations: deleteBook (DELETE /books/{id}), listBooks (GET /books)' "$summary" >/dev/null
grep -F 'Source: handlers/<books>.go:42' "$summary" >/dev/null
artifact_name="$(sed -n 's/^artifact-name=//p' "$output")"
grep -Ex 'artifact-name=gnr8-api-changes-test-[0-9a-f]{8}' "$output" >/dev/null
grep -Fx "<!-- gnr8-api-changes:$artifact_name -->" "$summary" >/dev/null
grep -Fx "marker=<!-- gnr8-api-changes:$artifact_name -->" "$output" >/dev/null
if grep -E '^## injected heading$|^```' "$summary" >/dev/null; then
  echo "report content escaped its indented code block" >&2
  exit 1
fi
# Both reports come from the CLI, each asked for exactly one format.
grep -F -- '--json changes --base HEAD' "$log" >/dev/null
grep -E -- '^changes --base HEAD .*--markdown $' "$log" >/dev/null
grep -F -- '--exempt-tag internal --exempt-tag internal' "$log" >/dev/null
grep -F -- '--exempt-tag \#partner' "$log" >/dev/null
grep -F -- '--exempt-tag \ partner\ APIs\ ' "$log" >/dev/null
grep -F -- '--gate-operation POST\ /books --gate-operation GET\ /reports' "$log" >/dev/null
grep -F -- '--acceptance-file reviewed-api-changes.json' "$log" >/dev/null
report_root="$(sed -n 's/^report-root=//p' "$output")"
test -s "$report_root/001/report.json"
test -s "$report_root/001/report.md"

# The default configuration exempts nothing, which leaves the tag loop appending no arguments at
# all. Expanding an empty array under `set -u` is fatal on bash 3.2 (GitHub's macOS runners), so
# the empty case must run the binary, not merely parse.
empty_output="$tmp/output-empty"
empty_summary="$tmp/summary-empty"
empty_log="$tmp/args-empty"
: > "$empty_output"
: > "$empty_summary"
: > "$empty_log"

GNR8_BIN="$fake" \
BASE_REF=HEAD \
EXEMPT_TAGS="" \
WORKING_DIRECTORIES="$weird_dir" \
RUNNER_TEMP="$tmp" \
GITHUB_OUTPUT="$empty_output" \
GITHUB_STEP_SUMMARY="$empty_summary" \
GITHUB_JOB=test \
FAKE_LOG="$empty_log" \
  "$runner" > "$tmp/run-stdout"

grep -Fx 'gating=true' "$empty_output" >/dev/null
grep -Fx 'breaking-count=1' "$empty_output" >/dev/null
grep -F -- '--base HEAD' "$empty_log" >/dev/null
if grep -F -- '--exempt-tag' "$empty_log" >/dev/null; then
  echo "empty exempt-tags must not pass --exempt-tag" >&2
  exit 1
fi
if grep -F -- '--gate-operation' "$empty_log" >/dev/null; then
  echo "empty gate-operations must not pass --gate-operation" >&2
  exit 1
fi
if grep -F -- '--acceptance-file' "$empty_log" >/dev/null; then
  echo "empty acceptance-file must not pass --acceptance-file" >&2
  exit 1
fi
# The heading is the one value this script renders, so it escapes it.
grep -F 'a&lt;b&gt;&amp;c' "$empty_summary" >/dev/null
if grep -F 'a<b>&c' "$empty_summary" >/dev/null; then
  echo "working directory reached the heading unescaped" >&2
  exit 1
fi

# Separate matrix invocations own distinct markers, each derived from its artifact name.
second_name="$(sed -n 's/^artifact-name=//p' "$empty_output")"
test "$artifact_name" != "$second_name"
grep -Fx "<!-- gnr8-api-changes:$second_name -->" "$empty_summary" >/dev/null

stderr="$tmp/missing-stderr"
if GNR8_BIN="$fake" \
  BASE_REF=refs/heads/not-present \
  WORKING_DIRECTORIES="$repo_root/examples/bookstore" \
  RUNNER_TEMP="$tmp" \
  GITHUB_OUTPUT="$output" \
  GITHUB_STEP_SUMMARY="$summary" \
  FAKE_LOG="$log" \
  "$runner" > "$tmp/run-stdout" 2> "$stderr"; then
  echo "expected missing base history to fail" >&2
  exit 1
fi
grep -F 'checkout with fetch-depth: 0' "$stderr" >/dev/null

# A complete first project survives a failed second project. Only completion outputs stay absent.
: > "$output"
: > "$summary"
if GNR8_BIN="$fake" BASE_REF=HEAD FAIL_ON_BREAKING=false FAIL_PROJECT="$weird_dir" \
  WORKING_DIRECTORIES="$repo_root/examples/bookstore"$'\n'"$weird_dir" \
  RUNNER_TEMP="$tmp" GITHUB_OUTPUT="$output" GITHUB_STEP_SUMMARY="$summary" \
  GITHUB_JOB=test FAKE_LOG="$log" "$runner" > "$tmp/run-stdout" 2> "$stderr"; then
  echo "expected project 2 to fail" >&2; exit 1
fi
grep -F 'report-root=' "$output" >/dev/null
grep -F 'artifact-name=' "$output" >/dev/null
grep -F 'BREAKING  DELETE /books/{id}' "$summary" >/dev/null
assert_absent -E '^(combined-report|gating)=' "$output"
report_root="$(sed -n 's/^report-root=//p' "$output")"
test -s "$report_root/001/report.md"
test -s "$report_root/001/report.json"

# Advisory mode accepts the CLI's status 1, keeps every publication output, and downgrades a
# protected breaking annotation to a warning. Status 2 above remains a hard failure in this mode.
: > "$output"
: > "$summary"
GNR8_BIN="$fake" BASE_REF=HEAD FAIL_ON_BREAKING=false \
  WORKING_DIRECTORIES="$repo_root/examples/bookstore" RUNNER_TEMP="$tmp" \
  GITHUB_OUTPUT="$output" GITHUB_STEP_SUMMARY="$summary" FAKE_LOG="$log" \
  "$runner" > "$tmp/advisory"
grep -Fx 'gating=true' "$output" >/dev/null
grep -Fx 'gating-count=1' "$output" >/dev/null
grep -F 'Enforcement: advisory' "$summary" >/dev/null
grep -F '::warning file=' "$tmp/advisory" >/dev/null
assert_absent -F '::error file=' "$tmp/advisory"
report_root="$(sed -n 's/^report-root=//p' "$output")"
test -s "$report_root/001/report.json"
test -s "$report_root/001/report.md"
test -s "$(sed -n 's/^combined-report=//p' "$output")"

# Keep both full artifacts when the summary budget cannot accommodate a whole project block.
: > "$output"
: > "$summary"
GNR8_BIN="$fake" BASE_REF=HEAD OVERSIZED=true \
  WORKING_DIRECTORIES="$repo_root/examples/bookstore"$'\n'"$weird_dir" \
  RUNNER_TEMP="$tmp" GITHUB_OUTPUT="$output" GITHUB_STEP_SUMMARY="$summary" \
  GITHUB_JOB=test FAKE_LOG="$log" "$runner" > "$tmp/run-stdout"
grep -F 'Report truncated at 900 KiB' "$summary" >/dev/null
test "$(grep -c 'Report truncated' "$summary")" -eq 1
test "$(wc -c < "$summary")" -lt $((1024 * 1024))
grep -Fx 'gating=true' "$output" >/dev/null
report_root="$(sed -n 's/^report-root=//p' "$output")"
test "$(wc -c < "$report_root/report.md")" -gt $((1800 * 1024))
test "$(grep -c '^<!-- gnr8-api-changes:' "$report_root/report.md")" -eq 2

# The explicit off switch emits no annotations while retaining the gate.
: > "$output"
: > "$summary"
GNR8_BIN="$fake" BASE_REF=HEAD ANNOTATE_API_CHANGES=false \
  WORKING_DIRECTORIES="$repo_root/examples/bookstore" RUNNER_TEMP="$tmp" \
  GITHUB_OUTPUT="$output" GITHUB_STEP_SUMMARY="$summary" FAKE_LOG="$log" \
  "$runner" > "$tmp/off"
assert_absent -E '^::(error|warning|notice)' "$tmp/off"
grep -Fx 'gating=true' "$output" >/dev/null

# An interpreter prerequisite is explicit; no second JSON-reading path exists.
mkdir -p "$tmp/no-python"
for tool in bash git cut mktemp cat wc dirname mkdir sed; do
  ln -s "$(command -v "$tool")" "$tmp/no-python/$tool"
done
: > "$output"
if PATH="$tmp/no-python" GNR8_BIN="$fake" BASE_REF=HEAD ANNOTATE_API_CHANGES=true \
  WORKING_DIRECTORIES="$repo_root/examples/bookstore" RUNNER_TEMP="$tmp" \
  GITHUB_OUTPUT="$output" GITHUB_STEP_SUMMARY="$summary" FAKE_LOG="$log" \
  "$runner" > "$tmp/run-stdout" 2> "$stderr"; then exit 1; fi
grep -F 'annotate-api-changes requires python3' "$stderr" >/dev/null

# Disabled annotations also work without the interpreter.
: > "$output"
PATH="$tmp/no-python" GNR8_BIN="$fake" BASE_REF=HEAD ANNOTATE_API_CHANGES=false \
  WORKING_DIRECTORIES="$repo_root/examples/bookstore" RUNNER_TEMP="$tmp" \
  GITHUB_OUTPUT="$output" GITHUB_STEP_SUMMARY="$summary" FAKE_LOG="$log" \
  "$runner" > "$tmp/off"
grep -Fx 'gating=true' "$output" >/dev/null
assert_absent -E '^::(error|warning|notice)' "$tmp/off"

# Summary write and emitter failures still publish the completed gate and report path.
: > "$output"
GNR8_BIN="$fake" BASE_REF=HEAD INVALID_JSON=true \
  WORKING_DIRECTORIES="$repo_root/examples/bookstore" RUNNER_TEMP="$tmp" \
  GITHUB_OUTPUT="$output" GITHUB_STEP_SUMMARY="$tmp/missing/summary" FAKE_LOG="$log" \
  "$runner" > "$tmp/failures" 2> "$stderr"
grep -F 'could not publish the step summary' "$tmp/failures" >/dev/null
grep -F 'could not publish API change annotations' "$tmp/failures" >/dev/null
grep -Fx 'gating=true' "$output" >/dev/null
grep -F 'combined-report=' "$output" >/dev/null
assert_absent -E '^(breaking-changes|breaking-count|gating-count)=' "$output"

# Multiline runner paths cannot inject extra GitHub outputs.
multiline_temp="$tmp/line"$'\n'"gating=false"
mkdir -p "$multiline_temp"
: > "$output"
GNR8_BIN="$fake" BASE_REF=HEAD ANNOTATE_API_CHANGES=false \
  WORKING_DIRECTORIES="$repo_root/examples/bookstore" RUNNER_TEMP="$multiline_temp" \
  GITHUB_OUTPUT="$output" GITHUB_STEP_SUMMARY="$summary" FAKE_LOG="$log" \
  "$runner" > "$tmp/run-stdout"
python3 - "$output" <<'PYTHON'
from pathlib import Path
import sys
lines = iter(Path(sys.argv[1]).read_text().splitlines())
outputs = {}
for line in lines:
    if '<<' in line:
        name, delimiter = line.split('<<', 1)
        values = []
        for part in lines:
            if part == delimiter:
                break
            values.append(part)
        outputs[name] = '\n'.join(values)
    else:
        name, value = line.split('=', 1)
        outputs[name] = value
assert set(outputs) == {'report-root', 'artifact-name', 'marker', 'gating', 'breaking-changes',
                        'breaking-count', 'gating-count', 'combined-report', 'report-digest'}
assert outputs['gating'] == 'true'
assert '\ngating=false/' in outputs['report-root']
assert Path(outputs['combined-report']).is_file()
PYTHON

# A carriage return in a directory cannot split Markdown or workflow commands.
control_dir="$repo_root/target/gnr8-action-changes-test/a"$'\r'"## injected heading"
mkdir -p "$control_dir"
: > "$output"
: > "$summary"
GNR8_BIN="$fake" BASE_REF=HEAD WORKING_DIRECTORIES="$control_dir" RUNNER_TEMP="$tmp" \
  GITHUB_OUTPUT="$output" GITHUB_STEP_SUMMARY="$summary" FAKE_LOG="$log" \
  "$runner" > "$tmp/control"
assert_absent -F $'\r' "$summary" "$tmp/control"
assert_absent -E '^## injected heading$' "$summary"
grep -Fx '::group::gnr8 changes project 1' "$tmp/control" >/dev/null
grep -F '%0D## injected heading/' "$tmp/control" >/dev/null

# Action metadata and its reference table expose the same advisory/filter contract. The default
# remains enforcing, and only the final status-1 gate step observes fail-on-breaking.
python3 - "$repo_root/action.yml" "$repo_root/docs/operations/artifacts-and-ci.md" <<'PYTHON'
from pathlib import Path
import re, sys
action = Path(sys.argv[1]).read_text()
docs = Path(sys.argv[2]).read_text()
inputs = action.split('\noutputs:\n', 1)[0].split('\ninputs:\n', 1)[1]
outputs = action.split('\noutputs:\n', 1)[1].split('\nruns:\n', 1)[0]
input_names = set(re.findall(r'^  ([a-z][a-z0-9-]*):$', inputs, re.M))
output_names = set(re.findall(r'^  ([a-z][a-z0-9-]*):$', outputs, re.M))
for name in ('fail-on-breaking', 'gate-operations', 'acceptance-file'):
    assert name in input_names
    assert f'| `{name}` |' in docs
for name in ('breaking-changes', 'breaking-count', 'gating-count', 'report-artifact',
             'report-path', 'report-digest'):
    assert name in output_names
    assert f'`{name}`' in docs
fail_block = re.search(r'^  fail-on-breaking:\n(.*?)(?=^  [a-z][a-z0-9-]*:|\Z)',
                       inputs, re.M | re.S).group(1)
assert 'default: "true"' in fail_block
gate_step = action.split('    - name: Enforce API change gate\n', 1)[1]
assert "inputs.fail-on-breaking == 'true'" in gate_step.split('\n      shell:', 1)[0]
assert 'exit 1' in gate_step
PYTHON

echo "action changes tests: OK (14 cases)"

#!/usr/bin/env bash
# Enforce AGENTS.md rule 0: gnr8 has exactly one native contract.
#
# No annotation dialects from other tools, no brownfield/compatibility product surface, no
# vocabulary that reads as "we support their thing". This is a hard gate, not a warning.
#
# Scope is ACTIVE product code and documentation. `.planning/` and `thoughts/` are historical
# evidence of past decisions and are deliberately excluded.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

status=0

# Paths searched. Everything else (.planning, thoughts, target, node_modules, .git) is out of scope.
scope=(
  README.md
  PLAN.md
  RELEASE-READINESS-V2.md
  llms.txt
  llms-full.txt
  action.yml
  Makefile
  # Adding a dependency is the most likely way foreign-generator coupling enters, so the
  # manifests are in scope, not just the source.
  Cargo.toml
  Cargo.lock
  crates
  docs
  examples
  fixtures
  goextract
  pyextract
  tsextract
  scripts
  .github
)

# Files exempt from every rule below, with the reason they exist.
#
#   scripts/check-invariants.sh  — this file necessarily names what it forbids.
#   AGENTS.md                    — the invariant text itself.
#   CHANGELOG.md                 — a record of what was REMOVED must name the removed symbols; it
#                                  is evidence of the deletion, the same category as `.planning/`.
is_exempt_file() {
  case "$1" in
    scripts/check-invariants.sh | AGENTS.md | CHANGELOG.md) return 0 ;;
    *) return 1 ;;
  esac
}

# Run one rule. Any surviving hit fails the gate.
#
#   $1 human-readable rule name
#   $2 extended regex of forbidden matches
#   $3 extended regex of legitimate uses to subtract (pass '' for none)
check_rule() {
  local name="$1" pattern="$2" allow="$3"
  local hits
  hits="$(grep -rInE --binary-files=without-match \
    --exclude-dir=node_modules --exclude-dir=target --exclude-dir=.git \
    -e "$pattern" "${scope[@]}" 2>/dev/null || true)"

  if [[ -n "$allow" ]]; then
    hits="$(printf '%s\n' "$hits" | { grep -vE "$allow" || true; })"
  fi

  # Drop exempt files.
  local surviving=""
  while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    is_exempt_file "${line%%:*}" && continue
    surviving+="$line"$'\n'
  done <<<"$hits"

  if [[ -n "${surviving//[$'\n']/}" ]]; then
    echo "INVARIANT VIOLATION — $name (AGENTS.md rule 0):" >&2
    printf '%s' "$surviving" >&2
    echo >&2
    status=1
  fi
}

# 0.1 — another tool's annotations, config files, or emitted packages.
#
# Naming a forbidden tool in order to state that we do NOT read it is the documented bright line
# (AGENTS.md 0.1), so disclaimer lines are subtracted. Anything that names one of these tools
# WITHOUT disclaiming it is a coupling risk and fails.
check_rule "foreign annotation/generator coupling" \
  '(openapi[-_. ]?generator|openapitools|swagger[-_]?codegen|oapi[-_]codegen|typescript[-_]axios|typescript[-_]fetch|antihax|swaggertype|swaggerignore|swaggo|drf[-_]yasg|drf[-_]spectacular|apispec|flasgger|springdoc|@nestjs/swagger|class-validator|class-transformer)' \
  '((never|Never|NEVER) |does not (read|consult|parse|treat|import)|doesn.t (read|consult|parse)|nor \*\*|, not erased|ignores (swagger|the)|forbidden by rule|third-party convention)'

# 0.2 / 0.3 — brownfield and compatibility as PRODUCT surface.
#
# Legitimate uses subtracted here:
#   - host/child protocol and wire/serialization compatibility (gnr8's own formats)
#   - "compatible"/"compatibility" describing runtime or toolchain requirements
#   - Swagger 2.0 / OpenAPI 3.x as SPEC FORMATS we import (tool-neutral)
check_rule "brownfield/compatibility product surface" \
  '(brownfield|SdkProfile|SdkTypeAliases|SdkOperationAliases|OpenApiSchemaAliases|clone_alias|GoExecuteCompatibility|GoRequestBuilderAliases|GoQuerySetterArgumentPolicy|RequiredPointerConstructorPolicy|TsModelPropertyPolicy|TsNullablePolicy|TsResponsePolicy|TsBarrelExports|legacy-bundle)' \
  ''

# 0.3 — vocabulary discipline for identifiers. Deliberately narrow: it targets DECLARATIONS
# (types, functions, modules, methods, CLI flags) rather than prose, so that legitimate protocol
# and wire compatibility discussion stays allowed.
check_rule "compat/legacy/brownfield vocabulary in identifiers" \
  '((pub )?(fn|mod|struct|enum|trait|const) [a-zA-Z_]*(compat|legacy|brownfield|migration|baseline)|fn [a-zA-Z_]*_(compat|legacy|migration|baseline)|--(compat|legacy|migration|baseline)\b|--profile[= ](minimal|native|compat|legacy)|def [a-z_]*(compat|legacy|migration|baseline)|(class|function) [a-zA-Z]*(Compat|Legacy|Brownfield|Migration|Baseline))' \
  '(protocol|handshake|PROTOCOL_VERSION|forward-compat)'

# Removed lifecycle surface must stay removed. The sole active-code occurrence is a clap regression
# test proving the old flag is rejected; historical release notes are exempt above.
check_rule "removed baseline-adoption surface" \
  '(--accept-generated-baseline|accept_generated_baseline|baseline_adopted)' \
  'try_parse_from.*--accept-generated-baseline.*is_err'

# 0.3 — the rules above grep file CONTENTS, so a forbidden name can hide in a path. An empty
# `fixtures/brownfield-openapi/` directory survived exactly that way.
forbidden_paths=""
for pattern in '*brownfield*' '*openapi-generator*' '*openapitools*' '*swagger-codegen*' '*compat*'; do
  hits="$(find "${scope[@]}" -name node_modules -prune -o -name target -prune -o -iname "$pattern" -print 2>/dev/null || true)"
  if [[ -n "${hits//[$'\n']/}" ]]; then
    forbidden_paths+="$hits"$'\n'
  fi
done
if [[ -n "${forbidden_paths//[$'\n']/}" ]]; then
  echo "INVARIANT VIOLATION — forbidden path name (AGENTS.md rule 0.3):" >&2
  printf '%s\n' "$forbidden_paths" >&2
  status=1
fi

if [[ $status -eq 0 ]]; then
  echo "invariants: clean — one native contract, no foreign coupling"
fi

exit "$status"

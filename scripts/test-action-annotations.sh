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
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
python3 - "$tmp/report.json" <<'PY'
import json, sys
changes = []
for kind, gating in [('breaking', True), ('breaking', False), ('additive', False), ('doc_only', False)]:
    changes.append(dict(kind=kind, gating=gating, code='request.property.required.added',
                        message='hostile\n%::,\rmessage', file='main.go', line=25, span=dict(end_line=25)))
changes.append(dict(kind='breaking', gating=True, code='operation.removed', message='removed'))
changes.extend([changes[0].copy() for _ in range(60)])
with open(sys.argv[1], 'w') as out:
    json.dump(dict(schema_version=1, changes=changes), out)
PY
python3 "$repo_root/scripts/emit-action-annotations.py" "$tmp/report.json" examples/bookstore test-artifact > "$tmp/commands"
grep -Fx '::error file=examples/bookstore/main.go,line=25,endLine=25,title=gnr8%3A request.property.required.added::hostile%0A%25::,%0Dmessage' "$tmp/commands" >/dev/null
grep -F '::warning file=examples/bookstore/main.go' "$tmp/commands" >/dev/null
grep -F '::notice file=examples/bookstore/main.go' "$tmp/commands" >/dev/null
test "$(grep -c '^::.* file=' "$tmp/commands")" -eq 50
grep -Fx '::notice::gnr8: 14 further findings not annotated (1 unanchorable); see the job summary and the "test-artifact" artifact.' "$tmp/commands" >/dev/null
test "$(wc -l < "$tmp/commands")" -eq 51
assert_absent -F 'operation.removed' "$tmp/commands"
python3 "$repo_root/scripts/emit-action-annotations.py" --advisory \
  "$tmp/report.json" examples/bookstore test-artifact > "$tmp/advisory-commands"
test "$(grep -c '^::warning file=' "$tmp/advisory-commands")" -eq 49
test "$(grep -c '^::notice file=' "$tmp/advisory-commands")" -eq 1
assert_absent -F '::error file=' "$tmp/advisory-commands"
printf '{"schema_version":2,"changes":[]}' > "$tmp/report.json"
if python3 "$repo_root/scripts/emit-action-annotations.py" "$tmp/report.json" . report > "$tmp/commands" 2> "$tmp/error"; then exit 1; fi
grep -F 'gnr8 action: cannot emit API change annotations' "$tmp/error" >/dev/null
test ! -s "$tmp/commands"
# Execute the action's real input validation with both accepted boolean spellings and a bad one.
python3 - "$repo_root/action.yml" "$tmp/validate-step" <<'PYTHON'
from pathlib import Path
import sys, textwrap
step = Path(sys.argv[1]).read_text().split('    - name: Validate gnr8 action inputs\n')[1].split('\n    - name: ')[0]
Path(sys.argv[2]).write_text(textwrap.dedent(step.split('      run: |\n')[1]))
PYTHON
export CACHE_ENABLED=false INPUT_BINARY=gnr8 INSTALL_METHOD=path REQUESTED_VERSION=lock
export REPORT_API_CHANGES=false BASE_REF=HEAD SETUP_GO=false SETUP_NODE=false SETUP_PYTHON=false SETUP_RUST=false
export WORKING_DIRECTORIES="$repo_root/examples/bookstore"
export FAIL_ON_BREAKING=true
ANNOTATE_API_CHANGES=false bash "$tmp/validate-step"
ANNOTATE_API_CHANGES=true bash "$tmp/validate-step"
if ANNOTATE_API_CHANGES=yes bash "$tmp/validate-step" 2> "$tmp/error"; then exit 1; fi
grep -F 'annotate-api-changes must be "true" or "false"' "$tmp/error" >/dev/null
if FAIL_ON_BREAKING=yes ANNOTATE_API_CHANGES=true bash "$tmp/validate-step" 2> "$tmp/error"; then exit 1; fi
grep -F 'fail-on-breaking must be "true" or "false"' "$tmp/error" >/dev/null

echo 'action annotation tests: OK (5 cases)'

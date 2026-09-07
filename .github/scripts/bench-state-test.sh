#!/usr/bin/env bash
set -euo pipefail
scripts="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
export GITHUB_OUTPUT="$scratch/output"
export GITHUB_REPOSITORY=owner/repo
export STATE_TEST_VALUE=""
export STATE_TEST_FAILURE=false
export BENCH_STATE_TOKEN=state-test-token
export GH_TOKEN=other-test-token
feature=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
previous=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
export STATE_TEST_DATE
STATE_TEST_DATE=$(date -u +%Y-%m-%dT%H:%M:%SZ)

gh() {
  case "$1" in
    api)
      [[ "$GH_TOKEN" == state-test-token && "$*" == *'--paginate'* ]] || return 1
      [[ "$STATE_TEST_FAILURE" == false ]] || return 1
      printf '%s\n' "$STATE_TEST_VALUE"
      ;;
    variable)
      [[ "$STATE_TEST_FAILURE" == false ]] || return 1
      [[ "$*" == 'variable set BENCH_TEST_LAST_FEATURE_REF --repo owner/repo --body aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' ]]
      ;;
    run)
      if [[ "$*" == *'--status=in_progress'* ]]; then
        printf '0\n'
      else
        printf '[{"headSha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","createdAt":"%s","conclusion":"success"}]\n' "$STATE_TEST_DATE"
      fi
      ;;
    *) return 1 ;;
  esac
}
git() {
  case "$*" in
    fetch*) ;;
    'rev-parse origin/main') printf '%s\n' aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ;;
    'rev-parse origin/main~1') printf '%s\n' bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb ;;
    'log -1 --format=%cI '* ) printf '%s\n' "$STATE_TEST_DATE" ;;
    *) return 1 ;;
  esac
}
export -f gh git

[[ -z "$(bash "$scripts/bench-state.sh" get owner/repo BENCH_TEST_LAST_FEATURE_REF)" ]]
export STATE_TEST_VALUE="$previous"
[[ "$(bash "$scripts/bench-state.sh" get owner/repo BENCH_TEST_LAST_FEATURE_REF)" == "$previous" ]]
bash "$scripts/bench-state.sh" set owner/repo BENCH_TEST_LAST_FEATURE_REF "$feature"
export STATE_TEST_VALUE=invalid
if bash "$scripts/bench-state.sh" get owner/repo BENCH_TEST_LAST_FEATURE_REF 2>/dev/null; then exit 1; fi
if bash "$scripts/bench-state.sh" set owner/repo BENCH_TEST_LAST_FEATURE_REF invalid 2>/dev/null; then exit 1; fi
export STATE_TEST_FAILURE=true
if bash "$scripts/bench-state.sh" get owner/repo BENCH_TEST_LAST_FEATURE_REF 2>/dev/null; then exit 1; fi
if bash "$scripts/bench-state.sh" set owner/repo BENCH_TEST_LAST_FEATURE_REF "$feature" 2>/dev/null; then exit 1; fi
export STATE_TEST_FAILURE=false
echo 'PASS: missing state, SHA validation, credential selection, and API failures'

for resolver in bench-scheduled-refs.sh bench-replay-scheduled-refs.sh bench-e2e-scheduled-refs.sh; do
  [[ -f "$scripts/$resolver" ]] || continue
  args=(false)
  [[ "$resolver" != bench-scheduled-refs.sh ]] || args+=(hourly)
  for value in "" "$feature" "$previous"; do
    export STATE_TEST_VALUE="$value"
    : > "$GITHUB_OUTPUT"
    bash "$scripts/$resolver" "${args[@]}" > "$scratch/log" 2>&1
    if [[ "$value" == "$feature" ]]; then
      rg -q '^should-skip=true$' "$GITHUB_OUTPUT"
    else
      rg -q '^should-skip=false$' "$GITHUB_OUTPUT"
    fi
    if [[ "$value" == "$previous" ]]; then
      rg -q "^baseline-ref=$previous$" "$GITHUB_OUTPUT"
    fi
  done
  export STATE_TEST_FAILURE=true
  if bash "$scripts/$resolver" "${args[@]}" > "$scratch/log" 2>&1; then exit 1; fi
  export STATE_TEST_FAILURE=false
  echo "PASS: $resolver first run, unchanged/changed commit, and API failure"
done

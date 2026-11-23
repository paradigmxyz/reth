#!/usr/bin/env bash
#
# Runs amp with --stream-json, logs its thread URL and prints only the final result, so callers can
# capture it. Exits non-zero when Amp fails (Update evm2 workflow).
set -uo pipefail

OUTPUT=$(amp "$@" --stream-json 2>&1)
AMP_EXIT=$?
THREAD_ID=$(echo "$OUTPUT" | head -1 | jq -r '.session_id // empty' 2>/dev/null)
if [ -n "$THREAD_ID" ]; then
  echo "Amp thread: https://ampcode.com/threads/${THREAD_ID}" >&2
fi
echo "$OUTPUT" | jq -r 'select(.type=="result") | .result // empty' 2>/dev/null
IS_ERROR=$(echo "$OUTPUT" | jq -r 'select(.type=="result") | .is_error // false' 2>/dev/null)
if [ "$AMP_EXIT" -ne 0 ] || [ "$IS_ERROR" = "true" ]; then
  echo "::group::Amp errors" >&2
  echo "$OUTPUT" | jq -r 'select(.type=="error") | .message // empty' 2>/dev/null >&2
  printf '%s\n' "$OUTPUT" | awk 'substr($0,1,1)!="{" { print }' >&2
  echo "::endgroup::" >&2
  exit 1
fi

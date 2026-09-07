#!/usr/bin/env bash
set -euo pipefail
script="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/bench-upload-charts.sh"
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/charts with spaces" "$scratch/empty"
# A 1x1 PNG; no credentials or network are needed by these tests.
printf '%s' 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=' |
  base64 -d > "$scratch/charts with spaces/latency_throughput.png"
export UPLOAD_TEST_LOG="$scratch/calls"
export UPLOAD_TEST_MODE=dry
gh() {
  printf '%s\n' "$*" >> "$UPLOAD_TEST_LOG"
  [[ "$UPLOAD_TEST_MODE" != dry ]]
  if [[ "$2" == repos/owner/repo ]]; then
    printf '123\n'
    return
  fi
  [[ "$2" == https://uploads.github.com/user-attachments/assets ]]
  [[ "$*" == *'content_type=image/png'* && "$*" == *'repository_id=123'* ]]
  [[ "$*" == *'Content-Type: application/octet-stream'* ]]
  if [[ "$UPLOAD_TEST_MODE" == failure ]]; then
    echo 'HTTP 401' >&2
    return 1
  fi
  printf '{"url":"https://github.com/user-attachments/assets/test-image"}\n'
}
export -f gh
bash "$script" --dry-run owner/repo "$scratch/charts with spaces" > "$scratch/dry-run"
[[ ! -e "$UPLOAD_TEST_LOG" ]]
[[ $(wc -l < "$scratch/dry-run") == 2 ]]
grep -q 'repository_id=REPOSITORY_ID' "$scratch/dry-run"
grep -q 'uploads.github.com/user-attachments/assets' "$scratch/dry-run"
echo 'PASS: dry run validates files and prints requests without calling gh'

export UPLOAD_TEST_MODE=success
bash "$script" owner/repo "$scratch/charts with spaces" > "$scratch/charts.md"
grep -Fq '![Latency, Throughput & Diff](https://github.com/user-attachments/assets/test-image)' "$scratch/charts.md"
[[ $(wc -l < "$UPLOAD_TEST_LOG") == 2 ]]
echo 'PASS: upload request and rendered Markdown'

export UPLOAD_TEST_MODE=failure
if bash "$script" owner/repo "$scratch/charts with spaces" > /dev/null 2>&1; then
  echo 'Expected upload failure' >&2; exit 1
fi
[[ $(wc -l < "$UPLOAD_TEST_LOG") == 4 ]]
echo 'PASS: upload errors propagate without retries'

if bash "$script" --dry-run owner/repo "$scratch/empty" > /dev/null 2>&1; then
  echo 'Expected missing charts failure' >&2; exit 1
fi
printf invalid > "$scratch/charts with spaces/bad.png"
if bash "$script" --dry-run owner/repo "$scratch/charts with spaces" > /dev/null 2>&1; then
  echo 'Expected invalid PNG failure' >&2; exit 1
fi
[[ $(wc -l < "$UPLOAD_TEST_LOG") == 4 ]]
echo 'PASS: missing or invalid charts fail before network calls'

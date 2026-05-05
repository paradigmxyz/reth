#!/usr/bin/env bash
set -euo pipefail

export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
echo "$HOME/.local/bin" >> "$GITHUB_PATH"
echo "$HOME/.cargo/bin" >> "$GITHUB_PATH"

missing=()
for cmd in schelk cpupower taskset stdbuf python3 curl make uv jq; do
  command -v "$cmd" &>/dev/null || missing+=("$cmd")
done
if [ ${#missing[@]} -gt 0 ]; then
  echo "::error::Missing required tools: ${missing[*]}"
  exit 1
fi
echo "All dependencies found"

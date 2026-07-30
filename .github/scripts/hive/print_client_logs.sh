#!/usr/bin/env bash
set -euo pipefail

shopt -s nullglob

truncate_lines() {
    cut -c 1-2000
}

for log in hivetests/workspace/logs/reth/client-*.log; do
    lines="$(wc -l < "$log")"
    bytes="$(wc -c < "$log")"

    echo "::group::$log"
    echo "lines=${lines} bytes=${bytes}"

    echo "first 200 lines:"
    sed -n '1,200p' "$log" | truncate_lines

    echo "error context:"
    rg -n -i -C 20 \
        'panic|fatal|error|warn|oom|out of memory|killed|no route to host|connection refused|cannot allocate' \
        "$log" | head -n 2000 | truncate_lines || true

    echo "last 300 lines:"
    tail -n 300 "$log" | truncate_lines

    echo "::endgroup::"
done

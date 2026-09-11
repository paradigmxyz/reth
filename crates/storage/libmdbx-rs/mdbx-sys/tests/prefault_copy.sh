#!/usr/bin/env bash
set -euo pipefail
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
test_dir=$(mktemp -d)
trap 'rm -rf -- "$test_dir"' EXIT
"${CC:-cc}" -std=c11 -O1 -g -DMDBX_DEBUG=1 -DMDBX_USE_FALLOCATE=0 \
  ${CFLAGS:-} "$script_dir/prefault_copy.c" -pthread -lm -o "$test_dir/prefault-copy"
"$test_dir/prefault-copy"

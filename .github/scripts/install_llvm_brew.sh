#!/usr/bin/env bash
set -eo pipefail

v=${1:-22}

brew install "llvm@${v}"
prefix="$(brew --prefix "llvm@${v}")"
echo "LLVM_SYS_${v}1_PREFIX=${prefix}" >> "$GITHUB_ENV"
echo "${prefix}/bin" >> "$GITHUB_PATH"

echo "LLVM $v installed:"
"${prefix}/bin/llvm-config" --version

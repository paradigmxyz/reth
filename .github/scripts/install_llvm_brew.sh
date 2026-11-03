#!/usr/bin/env bash
set -eo pipefail

v=${1:-23}

brew install "llvm@${v}"
prefix="$(brew --prefix "llvm@${v}")"
if [[ -n "${GITHUB_ENV:-}" ]]; then
    echo "LLVM_SYS_${v}1_PREFIX=${prefix}" >> "$GITHUB_ENV"
    echo "LLVM_CONFIG_PATH=${prefix}/bin/llvm-config" >> "$GITHUB_ENV"
fi
if [[ -n "${GITHUB_PATH:-}" ]]; then
    echo "${prefix}/bin" >> "$GITHUB_PATH"
fi

echo "LLVM $v installed:"
"${prefix}/bin/llvm-config" --version

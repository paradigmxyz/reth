#!/usr/bin/env bash
set -eo pipefail

os=${1:?usage: install_llvm.sh <ubuntu|macos> [version]}
version=${2:-23}
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

case "$os" in
    ubuntu)
        sudo "$script_dir/install_llvm_ubuntu.sh" "$version"
        prefix="$(llvm-config --prefix)"
        version_major_minor="$(llvm-config --version | awk -F. '{ print $1 $2 }')"
        if [[ -n "${GITHUB_ENV:-}" ]]; then
            echo "LLVM_SYS_${version_major_minor}_PREFIX=${prefix}" >> "$GITHUB_ENV"
            echo "LLVM_CONFIG_PATH=$(which llvm-config)" >> "$GITHUB_ENV"
        fi
        ;;
    macos)
        "$script_dir/install_llvm_brew.sh" "$version"
        ;;
    *)
        echo "unsupported OS: $os" >&2
        exit 1
        ;;
esac

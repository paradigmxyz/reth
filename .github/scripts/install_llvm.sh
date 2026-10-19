#!/usr/bin/env bash
set -eo pipefail

os=${1:?usage: install_llvm.sh <ubuntu|macos> [version]}
version=${2:-22}
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

case "$os" in
    ubuntu)
        sudo "$script_dir/install_llvm_ubuntu.sh" "$version"
        ;;
    macos)
        "$script_dir/install_llvm_brew.sh" "$version"
        ;;
    *)
        echo "unsupported OS: $os" >&2
        exit 1
        ;;
esac

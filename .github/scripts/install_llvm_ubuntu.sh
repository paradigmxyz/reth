#!/usr/bin/env bash
set -eo pipefail

v=${1:-22}
bins=(clang llvm-config lld ld.lld FileCheck)

# Install prerequisites for llvm.sh.
# software-properties-common is needed on older distros (bookworm) for add-apt-repository
# but not on newer ones (trixie, forky) where llvm.sh uses a different method.
apt-get update -qq
apt-get install -y --no-install-recommends \
    lsb-release wget gnupg ca-certificates
apt-get install -y --no-install-recommends software-properties-common 2>/dev/null || true

# Use the official LLVM install script which handles distro detection,
# GPG key import, and apt source configuration for all Debian/Ubuntu versions.
llvm_sh=$(mktemp)
wget -qO "$llvm_sh" https://apt.llvm.org/llvm.sh
chmod +x "$llvm_sh"
"$llvm_sh" "$v" all
rm -f "$llvm_sh"

for bin in "${bins[@]}"; do
    if ! command -v "$bin-$v" &>/dev/null; then
        echo "Warning: $bin-$v not found" 1>&2
        continue
    fi
    ln -fs "$(which "$bin-$v")" "/usr/bin/$bin"
done

echo "LLVM $v installed:"
llvm-config --version

#!/usr/bin/env bash
set -eo pipefail

v=${1:-23}
# `dev` installs only what building against LLVM $v needs, next to the default LLVM, and
# leaves the default symlinks alone. llvm.sh cannot add a second version because every
# python3-lldb-* package conflicts with the others.
mode=${2:-}
bins=(clang llvm-config lld ld.lld FileCheck)

# Install prerequisites for llvm.sh.
# software-properties-common is needed on older distros (bookworm) for add-apt-repository.
# It is not needed on newer distros (trixie, forky), where llvm.sh uses a different method.
apt-get update -qq
apt-get install -y --no-install-recommends \
    lsb-release wget gnupg ca-certificates
apt-get install -y --no-install-recommends software-properties-common 2>/dev/null || true

if [[ "$mode" == dev ]]; then
    codename=$(lsb_release -cs)
    wget -qO /etc/apt/trusted.gpg.d/apt.llvm.org.asc https://apt.llvm.org/llvm-snapshot.gpg.key
    echo "deb http://apt.llvm.org/$codename/ llvm-toolchain-$codename-$v main" >"/etc/apt/sources.list.d/llvm-$v.list"
    apt-get update -qq
    apt-get install -y --no-install-recommends "llvm-$v-dev" "libpolly-$v-dev"
    echo "LLVM $v installed:"
    "llvm-config-$v" --version --bindir --libdir
    exit 0
fi

# Use the official LLVM install script.
# It handles distro detection, GPG key import, and apt source configuration.
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
llvm-config --version --bindir --libdir

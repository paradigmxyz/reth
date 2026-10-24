#!/usr/bin/env bash
set -eo pipefail

v=${1:-23}
bins=(clang llvm-config lld ld.lld FileCheck)

apt-get update -qq
apt-get install -y --no-install-recommends \
    lsb-release wget ca-certificates

# Reuse an existing repository when this LLVM version is already available.
if ! apt-cache show "llvm-$v-dev" >/dev/null 2>&1; then
    codename=$(lsb_release -cs)
    suite="llvm-toolchain-$codename-$v"
    case "$codename" in
        sid|unstable|forky)
            codename=unstable
            suite="llvm-toolchain-$v"
            ;;
    esac
    key=/etc/apt/trusted.gpg.d/apt.llvm.org.asc
    wget -qO- https://apt.llvm.org/llvm-snapshot.gpg.key | tee "$key" >/dev/null
    echo "deb [signed-by=$key] https://apt.llvm.org/$codename/ $suite main" \
        > "/etc/apt/sources.list.d/llvm-$v.list"
    apt-get update -qq
fi

# llvm.sh installs LLDB even without "all". Its Python bindings conflict across
# versions on shared runners, so install only the compiler and build libraries.
# Keep Polly for llvm-sys's static linking of LLVM components.
apt-get install -y --no-install-recommends \
    "clang-$v" "lld-$v" "llvm-$v-dev" "llvm-$v-tools" \
    "libclang-$v-dev" "libclang-rt-$v-dev" "libpolly-$v-dev"

for bin in "${bins[@]}"; do
    if ! command -v "$bin-$v" &>/dev/null; then
        echo "Warning: $bin-$v not found" 1>&2
        continue
    fi
    ln -fs "$(which "$bin-$v")" "/usr/bin/$bin"
done

echo "LLVM $v installed:"
llvm-config --version --bindir --libdir

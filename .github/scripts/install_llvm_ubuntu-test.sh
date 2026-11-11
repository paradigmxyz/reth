#!/usr/bin/env bash
set -euo pipefail

script="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/install_llvm_ubuntu.sh"
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
export LLVM_TEST_LOG="$scratch/commands"
export LLVM_TEST_REPOSITORY_READY=false
export LLVM_TEST_CODENAME=noble
export LLVM_TEST_INSTALL_FAIL=false

# Redirect repository files and mock system commands to avoid changing the host.
mkdir -p "$scratch/apt/trusted.gpg.d" "$scratch/apt/sources.list.d" "$scratch/bin"
sed "s|/etc/apt|$scratch/apt|g" "$script" > "$scratch/install.sh"
for bin in clang llvm-config lld ld.lld FileCheck; do
    for version in 22 23; do
        printf '#!/usr/bin/env bash\nexit 0\n' > "$scratch/bin/$bin-$version"
        chmod +x "$scratch/bin/$bin-$version"
    done
done
export PATH="$scratch/bin:$PATH"

apt-get() {
    printf 'apt-get %s\n' "$*" >> "$LLVM_TEST_LOG"
    # Reproduce the runner failure if a debugger package is requested.
    [[ "$*" != *lldb* ]] || return 100
    if [[ "$LLVM_TEST_INSTALL_FAIL" = true && "$*" = *clang-* ]]; then
        return 100
    fi
}
apt-cache() { [[ "$LLVM_TEST_REPOSITORY_READY" = true ]]; }
lsb_release() { echo "$LLVM_TEST_CODENAME"; }
wget() { echo 'test signing key'; }
ln() { printf 'ln %s\n' "$*" >> "$LLVM_TEST_LOG"; }
llvm-config() { :; }
export -f apt-get apt-cache lsb_release wget ln llvm-config

for codename in noble jammy bookworm trixie forky sid; do
    export LLVM_TEST_CODENAME="$codename"
    for version in 22 23; do
        : > "$LLVM_TEST_LOG"
        bash "$scratch/install.sh" "$version" > /dev/null
        suite="llvm-toolchain-$codename-$version"
        repo_codename="$codename"
        if [[ "$codename" = forky || "$codename" = sid ]]; then
            suite="llvm-toolchain-$version"
            repo_codename=unstable
        fi
        expected="deb [signed-by=$scratch/apt/trusted.gpg.d/apt.llvm.org.asc] https://apt.llvm.org/$repo_codename/ $suite main"
        [[ "$(cat "$scratch/apt/sources.list.d/llvm-$version.list")" = "$expected" ]]
        grep -Fx "apt-get install -y --no-install-recommends clang-$version lld-$version llvm-$version-dev llvm-$version-tools libclang-$version-dev libclang-rt-$version-dev libpolly-$version-dev" "$LLVM_TEST_LOG" > /dev/null
        [[ "$(grep -c '^ln ' "$LLVM_TEST_LOG")" = 5 ]]
    done
done

# Existing repository configuration must be preserved, including its signing options.
export LLVM_TEST_REPOSITORY_READY=true
echo 'existing repository' > "$scratch/apt/sources.list.d/llvm-22.list"
bash "$scratch/install.sh" > /dev/null
[[ "$(cat "$scratch/apt/sources.list.d/llvm-22.list")" = 'existing repository' ]]

export LLVM_TEST_INSTALL_FAIL=true
if bash "$scratch/install.sh" > /dev/null 2>&1; then
    echo 'FAIL: package installation errors must fail the installer' >&2
    exit 1
fi
echo 'PASS: LLVM installation avoids debugger packages and preserves repositories'

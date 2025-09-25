#!/usr/bin/env bash

# Installs Geth (https://geth.ethereum.org) in $HOME/bin for x86_64 Linux.

set -eo pipefail

GETH_BUILD=${GETH_BUILD:-"1.13.4-3f907d6a"}

name="geth-linux-amd64-$GETH_BUILD"
url="https://gethstore.blob.core.windows.net/builds/$name.tar.gz"
tarball="$name.tar.gz"

mkdir -p "$HOME/bin"
wget -O "$tarball" "$url"
# Optional integrity check: set GETH_SHA256 to expected checksum to verify the tarball.
# Example: export GETH_SHA256="<expected_sha256>"; the script will verify and abort on mismatch.
if [ -n "${GETH_SHA256:-}" ]; then
  echo "$GETH_SHA256  $tarball" | sha256sum -c -
fi

tar -xvf "$tarball"
rm "$tarball"
mv "$name/geth" "$HOME/bin/geth"
rm -rf "$name"
chmod +x "$HOME/bin/geth"

# Add $HOME/bin to $PATH
[[ "$PATH" != *$HOME/bin* ]] && export PATH=$HOME/bin:$PATH
[ -n "$CI" ] && echo "$HOME/bin" >> "$GITHUB_PATH"

geth version

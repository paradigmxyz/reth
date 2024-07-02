#!/usr/bin/env bash

# Installs Geth (https://geth.ethereum.org) in $HOME/bin for x86_64 Linux.

set -eo pipefail

GETH_BUILD=${GETH_BUILD:-"1.13.4-3f907d6a"}

name="geth-linux-amd64-$GETH_BUILD"

mkdir -p "$HOME/bin"
wget "https://gethstore.blob.core.windows.net/builds/$name.tar.gz"
tar -xvf "$name.tar.gz"
rm "$name.tar.gz"
mv "$name/geth" "$HOME/bin/geth"
rm -rf "$name"
chmod +x "$HOME/bin/geth"

# Add $HOME/bin to $PATH
[[ "$PATH" != *$HOME/bin* ]] && export PATH=$HOME/bin:$PATH
[ -n "$CI" ] && echo "$HOME/bin" >> "$GITHUB_PATH"

geth version

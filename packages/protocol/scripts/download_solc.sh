#!/bin/sh

set -e

protocol_dir=$(realpath "$(dirname $0)/..")
solc_bin=${protocol_dir}/bin/solc

if [ -f "${solc_bin}" ]; then
  exit 0
fi

mkdir -p "$(dirname ${solc_bin})"

VERSION=v0.8.18

if [ "$(uname)" = 'Darwin' ]; then
  SOLC_FILE_NAME=solc-macos
elif [ "$(uname)" = 'Linux' ]; then
  SOLC_FILE_NAME=solc-static-linux
else
  echo "unsupported platform $(uname)"
  exit 1
fi

wget -O "${solc_bin}" https://github.com/ethereum/solidity/releases/download/$VERSION/$SOLC_FILE_NAME

chmod +x "${solc_bin}"

#!/usr/bin/env bash
set -euo pipefail

mkdir -p "$HOME/.local/bin"

sudo apt-get update -qq
sudo apt-get install -y --no-install-recommends \
  python3 make jq zstd curl dmsetup \
  linux-tools-"$(uname -r)" || \
  sudo apt-get install -y --no-install-recommends linux-tools-generic

if ! command -v uv &>/dev/null; then
  curl -LsSf https://astral.sh/uv/install.sh | env UV_INSTALL_DIR="$HOME/.local/bin" sh
fi

git config --global url."https://x-access-token:${DEREK_TOKEN}@github.com/".insteadOf "https://github.com/"

if ! command -v era_invalidate &>/dev/null; then
  git clone --depth 1 https://github.com/jthornber/thin-provisioning-tools /tmp/tpt
  sudo make -C /tmp/tpt install
  rm -rf /tmp/tpt
fi

if ! sudo sh -c 'command -v schelk' &>/dev/null; then
  cargo install --git https://github.com/tempoxyz/schelk --locked
  sudo install "$HOME/.cargo/bin/schelk" /usr/local/bin/
fi

if [ "${BENCH_SAMPLY:-false}" = "true" ] && ! sudo sh -c 'command -v samply' &>/dev/null; then
  cargo install samply --git https://github.com/DaniPopes/samply --branch edge --locked
  sudo install "$HOME/.cargo/bin/samply" /usr/local/bin/
fi

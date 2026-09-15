#!/usr/bin/env bash
#
# Installs the txgen tools used by the benchmark workflows.
#
# Optional env:
#   TXGEN_REPO  – txgen repository URL (default: https://github.com/tempoxyz/txgen)
#   TXGEN_REF   – branch or commit to install instead of the default branch
set -euxo pipefail

TXGEN_REPO="${TXGEN_REPO:-https://github.com/tempoxyz/txgen}"

if ! command -v llvm-config &>/dev/null; then
  .github/scripts/install_llvm.sh ubuntu
fi

# txgen is private. Prefer the deploy key secret; fall back to token auth for
# local/manual runs. Use the git CLI so cargo honors the auth configuration.
if [ -n "${TXGEN_DEPLOY_KEY:-}" ]; then
  set +x
  mkdir -p "$HOME/.ssh"
  printf '%s\n' "$TXGEN_DEPLOY_KEY" > "$HOME/.ssh/txgen_deploy_key"
  chmod 600 "$HOME/.ssh/txgen_deploy_key"
  ssh-keyscan github.com >> "$HOME/.ssh/known_hosts" 2>/dev/null
  export GIT_SSH_COMMAND="ssh -i $HOME/.ssh/txgen_deploy_key -o IdentitiesOnly=yes"
  set -x
  TXGEN_REPO="${TXGEN_SSH_REPO:-ssh://git@github.com/tempoxyz/txgen.git}"
elif [ -n "${TXGEN_TOKEN:-${GH_PROJECT_TOKEN:-${DEREK_PAT:-${DEREK_TOKEN:-}}}}" ]; then
  AUTH_TOKEN="${TXGEN_TOKEN:-${GH_PROJECT_TOKEN:-${DEREK_PAT:-${DEREK_TOKEN:-}}}}"
  set +x
  git config --global url."https://x-access-token:${AUTH_TOKEN}@github.com/".insteadOf "https://github.com/"
  set -x
fi
export CARGO_NET_GIT_FETCH_WITH_CLI=true

INSTALL_ARGS=()
if [ -n "${TXGEN_REF:-}" ]; then
  if [[ "$TXGEN_REF" =~ ^[0-9a-f]{7,40}$ ]]; then
    INSTALL_ARGS+=(--rev "$TXGEN_REF")
  else
    INSTALL_ARGS+=(--branch "$TXGEN_REF")
  fi
  # A pinned ref must replace whatever build of the same version is installed.
  INSTALL_ARGS+=(--force)
fi
cargo install --git "$TXGEN_REPO" ${INSTALL_ARGS[@]+"${INSTALL_ARGS[@]}"} --locked txgen-ethereum bench-cli

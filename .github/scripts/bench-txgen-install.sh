#!/usr/bin/env bash
#
# Installs the txgen tools used by the benchmark workflows.
#
# Optional env:
#   TXGEN_REPO  – txgen repository URL (default: https://github.com/tempoxyz/txgen)
#   TXGEN_REV   – txgen git revision (default: 2c7c8e2)
set -euxo pipefail

TXGEN_REPO="${TXGEN_REPO:-https://github.com/tempoxyz/txgen}"
TXGEN_REV="${TXGEN_REV:-2c7c8e2}"

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

cargo install --git "$TXGEN_REPO" --rev "$TXGEN_REV" --locked txgen-ethereum bench-cli

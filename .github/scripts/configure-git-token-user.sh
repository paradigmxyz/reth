#!/usr/bin/env bash
#
# Configures a git repository to commit as the GitHub user associated with a token.
#
# Usage: configure-git-token-user.sh <repo-dir> <github-token>
set -euo pipefail

REPO_DIR="${1:?repo dir is required}"
GITHUB_TOKEN="${2:?github token is required}"

GITHUB_USER=$(curl -fsSL \
  -H "Authorization: Bearer ${GITHUB_TOKEN}" \
  -H "Accept: application/vnd.github+json" \
  -H "X-GitHub-Api-Version: 2022-11-28" \
  https://api.github.com/user)
GIT_USER_NAME=$(python3 -c 'import json, sys; print(json.load(sys.stdin)["login"])' <<< "${GITHUB_USER}")
GIT_USER_ID=$(python3 -c 'import json, sys; print(json.load(sys.stdin)["id"])' <<< "${GITHUB_USER}")

git -C "${REPO_DIR}" config user.name "${GIT_USER_NAME}"
git -C "${REPO_DIR}" config user.email "${GIT_USER_ID}+${GIT_USER_NAME}@users.noreply.github.com"

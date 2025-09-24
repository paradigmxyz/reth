#!/bin/bash
# Borrowed from EthStaker's prepare for the merge guide
# See https://github.com/eth-educators/ethstaker-guides/blob/main/docs/prepare-for-the-merge.md#configuring-a-jwt-token-file

set -euo pipefail  # Fail fast and safer bash defaults

SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )
JWT_DIR="${SCRIPT_DIR}/jwttoken"
JWT_FILE="${JWT_DIR}/jwt.hex"

# Ensure directory exists with restrictive permissions
mkdir -p "${JWT_DIR}"
chmod 700 "${JWT_DIR}"

# Ensure private permissions for newly created files
umask 077

if [[ ! -f "${JWT_FILE}" ]]
then
# Write 32-byte hex key without trailing newline
  openssl rand -hex 32 | tr -d "\n" > "${JWT_FILE}"
  chmod 600 "${JWT_FILE}"
else
  echo "${JWT_FILE} already exists!"
fi

#!/bin/bash
set -euo pipefail
# Borrowed from EthStaker's prepare for the merge guide
# See https://github.com/eth-educators/ethstaker-guides/blob/main/docs/prepare-for-the-merge.md#configuring-a-jwt-token-file

SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )
install -d -m 700 "${SCRIPT_DIR}/jwttoken"
JWT_FILE="${SCRIPT_DIR}/jwttoken/jwt.hex"
if [[ ! -f "$JWT_FILE" ]]
then
  umask 077
  openssl rand -hex 32 | tr -d "\n" > "$JWT_FILE"
  chmod 600 "$JWT_FILE"
else
  echo "$JWT_FILE already exists!"
fi

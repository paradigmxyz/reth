#!/bin/bash
# Borrowed from EthStaker's prepare for the merge guide
# See https://github.com/eth-educators/ethstaker-guides/blob/main/docs/prepare-for-the-merge.md#configuring-a-jwt-token-file

# The JWT secret grants access to the Engine API, so create the directory (0700) and
# the file (0600) owner-only. Existing files and directories are left untouched.
umask 077

SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )
mkdir -p "${SCRIPT_DIR}/jwttoken"
if [[ ! -f "${SCRIPT_DIR}/jwttoken/jwt.hex" ]]
then
  openssl rand -hex 32 | tr -d "\n" | tee > "${SCRIPT_DIR}/jwttoken/jwt.hex"
else
  echo "${SCRIPT_DIR}/jwttoken/jwt.hex already exists!"
fi

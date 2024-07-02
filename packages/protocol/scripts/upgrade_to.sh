#!/bin/sh

set -e
: "${FORK_URL:=http://localhost:8545}"

forge script script/upgrade/Upgrade$CONTRACT.s.sol:Upgrade$CONTRACT \
    --fork-url $FORK_URL \
    --broadcast \
    --ffi \
    -vvvv
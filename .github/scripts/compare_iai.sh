#!/usr/bin/env bash

# This script should be run on the main branch, after running the iai benchmarks on the target branch.
# If the main branch has a better iai performance, exits in error.
# It ignores L2 differences, since they seem hard to stabilize across runs.

set -eo pipefail

cargo bench -p reth-db --bench iai -F test-utils \
  | tee /dev/tty \
  | awk '/((L1)|(Ins)|(RAM)|(Est))+.*\(\+[1-9]+[0-9]*\..*%\)/{f=1} END{exit f}'

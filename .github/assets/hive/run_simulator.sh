#!/usr/bin/env bash
set -eo pipefail

cd hivetests/

sim="${1}"
limit="${2}"

# Run the hive command with the specified parameters
hive --sim "${sim}" --sim.limit "${limit}" --sim.parallelism 4 --client reth 2>&1 | tee /tmp/log | grep -Eq "suites=0"

# Check if no tests were run
if [ $? -eq 0 ]; then
    echo "no tests were run"
    exit 1
else
    # Check the last line of the log for "finished" or "tests failed"
    tail -n 1 /tmp/log | grep -Eq "finished|tests failed"
    exit $?
fi
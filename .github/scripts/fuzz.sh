#!/bin/bash

# Runs fuzz tests using `cargo test-fuzz`. These should only be run after a `cargo test` has been executed once: https://github.com/trailofbits/test-fuzz#usage

PACKAGE=$1
TEST_TIME=${2:-5}

# Gets the list of tests present in the package.
TESTS=$(cargo test-fuzz --list -p $PACKAGE | head -n -3 | tail -n+9 | cat - <(echo \"--list\"]) | cat - | jq -r ".[]")

for test in $TESTS
do
    set -x
    cargo test-fuzz --no-ui  -p "$PACKAGE" $test  -- -V $TEST_TIME
    set +x
done;
#!/bin/bash
# Runs fuzz tests using `cargo test-fuzz`.

PACKAGE=$1
TEST_TIME=${2:-5}

echo "::group::$PACKAGE"

echo Building corpus.
cargo test -p $PACKAGE --no-run --all-features

# Gets the list of tests present in the package.
TESTS=$(cargo test-fuzz --list -p $PACKAGE | head -n -3 | tail -n+9 | cat - <(echo \"--list\"]) | cat - | jq -r ".[]")

for test in $TESTS
do
    echo Running test: $test
    set -x
    cargo test-fuzz --no-ui  -p "$PACKAGE" $test  -- -V $TEST_TIME
    set +x
done;

echo "::endgroup::"
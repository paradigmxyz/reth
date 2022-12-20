#!/bin/bash
set -e
# Runs fuzz tests using `cargo test-fuzz`.

PACKAGE=$1
TEST_TIME=${2:-5}

echo Building corpus.
cargo test -p $PACKAGE

# We configure coverage after building a corpus to only include
# fuzz tests in the coverage report.
echo Configuring coverage.
source <(cargo llvm-cov show-env --export-prefix)
cargo llvm-cov clean --workspace

# Gets the list of tests present in the package.
TESTS=$(cargo test-fuzz --list -p $PACKAGE | head -n -3 | tail -n+9 | cat - <(echo \"--list\"]) | cat - | jq -r ".[]")

for test in $TESTS
do
    echo Running test: $test
    set -x
    cargo test-fuzz --no-ui --exact -p "$PACKAGE" $test -- -V $TEST_TIME
    set +x
done;

echo Building coverage report.
cargo llvm-cov report --lcov --output-path lcov.info

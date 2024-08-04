#!/bin/bash

TEST_NAME="test_save_blocks_single_block"
PACKAGE="reth-engine-tree"
TOTAL_RUNS=100
FAILED_RUNS=0

echo "Running $TEST_NAME in package $PACKAGE $TOTAL_RUNS times..."

for i in $(seq 1 $TOTAL_RUNS); do
    echo "Run $i/$TOTAL_RUNS"
    if ! RUST_LOG=debug cargo test -p $PACKAGE $TEST_NAME -- --nocapture; then
        FAILED_RUNS=$((FAILED_RUNS + 1))
        echo "Test failed on run $i"
    fi
done

echo "Test completed. Failed runs: $FAILED_RUNS out of $TOTAL_RUNS"
echo "Failure rate: $(echo "scale=2; $FAILED_RUNS * 100 / $TOTAL_RUNS" | bc)%"

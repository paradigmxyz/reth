#!/bin/bash

# This script should be run on the main branch, after running the iai benchmarks on the target branch.

# If the main branch has a better iai performance, exits in error.
cargo bench --package reth-db --bench iai | awk '/\(-[1-9]+[0-9]*\..*%\)/{f=1} END{exit f}'
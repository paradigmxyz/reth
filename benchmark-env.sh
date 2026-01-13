#!/bin/bash
# Benchmark environment configuration

# Hoodi datadirs
export HOODI_DATADIR_MDBX="/home/yk/.local/share/reth/hoodi-bench-mdbx"
export HOODI_DATADIR_ROCKS="/home/yk/.local/share/reth/hoodi-bench-rocks"

# Source reth datadir (for RPC)
export HOODI_DATADIR_SOURCE="/home/yk/.local/share/reth/hoodi"

# RPC URLs (source node provides blocks, benchmark nodes receive them)
export RPC_URL="http://localhost:8545"
export ENGINE_URL_MDBX="http://localhost:8551"
export ENGINE_URL_ROCKS="http://localhost:8552"
export WS_URL_MDBX="ws://localhost:8546"
export WS_URL_ROCKS="ws://localhost:8548"

# Benchmark parameters
# Start from 1000 blocks before current head (1,766,369)
export START_BLOCK=1765369
export END_BLOCK=1766369
export BLOCK_COUNT=1000

# JWT secret
export JWT_SECRET="/home/yk/.local/share/reth/hoodi/jwt.hex"

# Results directory
export RESULTS_DIR="/home/yk/tempo/full_rocks/benchmark-results"

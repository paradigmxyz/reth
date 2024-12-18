#!/usr/bin/env bash
set -eo pipefail

# Create the hive_assets directory
mkdir hive_assets/

cd hivetests
go build .

./hive -client reth # first builds and caches the client

# Run each hive command in the background for each simulator and wait
echo "Building images"
./hive -client reth --sim "pyspec" -sim.timelimit 1s || true &
./hive -client reth --sim "ethereum/engine" -sim.timelimit 1s || true &
./hive -client reth --sim "devp2p" -sim.timelimit 1s || true &
./hive -client reth --sim "ethereum/rpc-compat" -sim.timelimit 1s || true &
./hive -client reth --sim "smoke/genesis" -sim.timelimit 1s || true &
./hive -client reth --sim "smoke/network" -sim.timelimit 1s || true &
./hive -client reth --sim "ethereum/sync" -sim.timelimit 1s || true &
wait

# Run docker save in parallel and wait
echo "Saving images"
docker save hive/hiveproxy:latest -o ../hive_assets/hiveproxy.tar &
docker save hive/simulators/devp2p:latest -o ../hive_assets/devp2p.tar &
docker save hive/simulators/ethereum/engine:latest -o ../hive_assets/engine.tar &
docker save hive/simulators/ethereum/rpc-compat:latest -o ../hive_assets/rpc_compat.tar &
docker save hive/simulators/ethereum/pyspec:latest -o ../hive_assets/pyspec.tar &
docker save hive/simulators/smoke/genesis:latest -o ../hive_assets/smoke_genesis.tar &
docker save hive/simulators/smoke/network:latest -o ../hive_assets/smoke_network.tar &
docker save hive/simulators/ethereum/sync:latest -o ../hive_assets/ethereum_sync.tar &
wait

# Make sure we don't rebuild images on the CI jobs
git apply ../.github/assets/hive/no_sim_build.diff
go build .
mv ./hive ../hive_assets/

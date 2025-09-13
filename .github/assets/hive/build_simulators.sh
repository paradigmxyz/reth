#!/usr/bin/env bash
set -eo pipefail

# Create the hive_assets directory
mkdir hive_assets/

cd hivetests
go build .

./hive -client reth # first builds and caches the client

# Run each hive command in the background for each simulator and wait
echo "Building images"
./hive -client reth --sim "ethereum/eest" --sim.buildarg fixtures=https://github.com/ethereum/execution-spec-tests/releases/download/v4.4.0/fixtures_develop.tar.gz --sim.buildarg branch=v4.4.0 -sim.timelimit 1s || true &
./hive -client reth --sim "ethereum/engine" -sim.timelimit 1s || true &
./hive -client reth --sim "devp2p" -sim.timelimit 1s || true &
./hive -client reth --sim "ethereum/rpc-compat" -sim.timelimit 1s || true &
./hive -client reth --sim "smoke/genesis" -sim.timelimit 1s || true &
./hive -client reth --sim "smoke/network" -sim.timelimit 1s || true &
./hive -client reth --sim "ethereum/sync" -sim.timelimit 1s || true &
wait

# Run docker save in parallel, wait and exit on error
echo "Saving images"
saving_pids=( )
docker save hive/hiveproxy:latest -o ../hive_assets/hiveproxy.tar & saving_pids+=( $! )
docker save hive/simulators/devp2p:latest -o ../hive_assets/devp2p.tar & saving_pids+=( $! )
docker save hive/simulators/ethereum/engine:latest -o ../hive_assets/engine.tar & saving_pids+=( $! )
docker save hive/simulators/ethereum/rpc-compat:latest -o ../hive_assets/rpc_compat.tar & saving_pids+=( $! )
docker save hive/simulators/ethereum/eest/consume-engine:latest -o ../hive_assets/eest_engine.tar & saving_pids+=( $! )
docker save hive/simulators/ethereum/eest/consume-rlp:latest -o ../hive_assets/eest_rlp.tar & saving_pids+=( $! )
docker save hive/simulators/smoke/genesis:latest -o ../hive_assets/smoke_genesis.tar & saving_pids+=( $! )
docker save hive/simulators/smoke/network:latest -o ../hive_assets/smoke_network.tar & saving_pids+=( $! )
docker save hive/simulators/ethereum/sync:latest -o ../hive_assets/ethereum_sync.tar & saving_pids+=( $! )
for pid in "${saving_pids[@]}"; do
    wait "$pid" || exit
done

# Make sure we don't rebuild images on the CI jobs
git apply ../.github/assets/hive/no_sim_build.diff
go build .
mv ./hive ../hive_assets/

#!/usr/bin/env bash
set -eo pipefail

# EELS release fixtures cover Osaka, BPO and Amsterdam.
eels_tag="v21.0.2"
# Pin all consume simulators to framework f9488c7 and fixtures tests@v21.0.2.
enginex_tag="latest@sha256:5d7b968e729c59efdad9e26e6a21d4bd7033ebfe2f9af115dafb74526bbef99b"
wirex_tag="latest@sha256:019ec1de59fb4e7e9afce0797bc029079b74dc937bd11a7e02d61f828a27e924"

# Create the hive_assets directory
mkdir hive_assets/

cd hivetests
go build .

./hive -client reth # first builds and caches the client

# Run each hive command in the background for each simulator and wait
echo "Building images"
./hive -client reth --sim "ethereum/eels/consume-enginex" \
    --sim.buildarg tag="${enginex_tag}" \
    --sim.timelimit 1s || true &
./hive -client reth --sim "ethereum/eels/consume-wirex" \
    --sim.buildarg tag="${wirex_tag}" \
    --sim.timelimit 1s || true &
./hive -client reth --sim "ethereum/eels/execute-blobs" \
    --sim.buildarg tag="${eels_tag}" \
    --sim.buildarg fork=Amsterdam \
    --sim.timelimit 1s || true &
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
docker save hive/simulators/ethereum/eels/consume-enginex:latest -o ../hive_assets/eels_enginex.tar & saving_pids+=( $! )
docker save hive/simulators/ethereum/eels/consume-wirex:latest -o ../hive_assets/eels_wirex.tar & saving_pids+=( $! )
docker save hive/simulators/ethereum/eels/execute-blobs:latest -o ../hive_assets/eels_blobs.tar & saving_pids+=( $! )
docker save hive/simulators/smoke/genesis:latest -o ../hive_assets/smoke_genesis.tar & saving_pids+=( $! )
docker save hive/simulators/smoke/network:latest -o ../hive_assets/smoke_network.tar & saving_pids+=( $! )
docker save hive/simulators/ethereum/sync:latest -o ../hive_assets/ethereum_sync.tar & saving_pids+=( $! )
for pid in "${saving_pids[@]}"; do
    wait "$pid" || exit
done

# Make sure we don't rebuild images on the CI jobs
git apply ../.github/scripts/hive/no_sim_build.diff
go build .
mv ./hive ../hive_assets/

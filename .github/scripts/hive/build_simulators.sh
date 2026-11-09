#!/usr/bin/env bash
set -eo pipefail

# EELS release fixtures cover Osaka, BPO and Amsterdam.
eels_tag="v21.0.2"
# Pin the framework used by the mainnet Hive dashboard (636b5f1).
enginex_tag="latest@sha256:67825aa3e2a5c148dd640c48066c4527e28130774de1a8cbd6b6ce122c507c26"
# Wirex has no release tag yet. Pin its published image, which contains tests@v21.0.2.
wirex_tag="latest@sha256:885470ec513e0d6ff955d4becbd36d308d280ea4dd91cda538504ef3c92659f4"

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

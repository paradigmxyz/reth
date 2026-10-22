#!/usr/bin/env bash
set -eo pipefail

fixture_variant="${1:-amsterdam}"
eels_tag="v21.0.1"
eels_build_args=(--sim.buildarg tag="${eels_tag}")
eels_blob_build_args=("${eels_build_args[@]}")

case "${fixture_variant}" in
    amsterdam)
        eels_fork="Amsterdam"
        ;;
    osaka)
        eels_fork="Osaka"
        ;;
    bogota)
        # Heze fixtures are separate from the mainnet simulator image release.
        eels_fixtures="https://github.com/ethereum/execution-specs/releases/download/tests-heze-devnet%40v3.0.0/fixtures_heze-devnet.tar.gz"
        eels_branch="devnets/heze/3"
        eels_build_args=(--sim.buildarg fixtures="${eels_fixtures}" --sim.buildarg branch="${eels_branch}")
        eels_blob_build_args=(--sim.buildarg branch="${eels_branch}")
        eels_fork="Bogota"
        ;;
    *)
        echo "unknown hive fixture variant: ${fixture_variant}"
        exit 1
        ;;
esac

# Create the hive_assets directory
mkdir hive_assets/

cd hivetests

go build .

./hive -client reth # first builds and caches the client

# Build fixture-heavy Heze images serially to limit peak disk usage.
echo "Building images"
./hive -client reth --sim "ethereum/eels/consume-engine" \
    "${eels_build_args[@]}" \
    --sim.timelimit 1s || true
./hive -client reth --sim "ethereum/eels/consume-rlp" \
    "${eels_build_args[@]}" \
    --sim.timelimit 1s || true
./hive -client reth --sim "ethereum/eels/execute-blobs" \
    "${eels_blob_build_args[@]}" \
    --sim.buildarg fork="${eels_fork}" \
    --sim.timelimit 1s || true &
./hive -client reth --sim "ethereum/engine" -sim.timelimit 1s || true &
./hive -client reth --sim "devp2p" -sim.timelimit 1s || true &
./hive -client reth --sim "ethereum/rpc-compat" -sim.timelimit 1s || true &
./hive -client reth --sim "smoke/genesis" -sim.timelimit 1s || true &
./hive -client reth --sim "smoke/network" -sim.timelimit 1s || true &
./hive -client reth --sim "ethereum/sync" -sim.timelimit 1s || true &
wait

# Export one image at a time and compress the stream to limit peak disk usage.
echo "Saving images"
save_image() {
    docker save "$1" | gzip -1 > "../hive_assets/$2.tar.gz"
}
save_image hive/hiveproxy:latest hiveproxy
save_image hive/simulators/devp2p:latest devp2p
save_image hive/simulators/ethereum/engine:latest engine
save_image hive/simulators/ethereum/rpc-compat:latest rpc_compat
save_image hive/simulators/ethereum/eels/consume-engine:latest eels_engine
save_image hive/simulators/ethereum/eels/consume-rlp:latest eels_rlp
save_image hive/simulators/ethereum/eels/execute-blobs:latest eels_blobs
save_image hive/simulators/smoke/genesis:latest smoke_genesis
save_image hive/simulators/smoke/network:latest smoke_network
save_image hive/simulators/ethereum/sync:latest ethereum_sync

# Make sure we don't rebuild images on the CI jobs
git apply ../.github/scripts/hive/no_sim_build.diff
go build .
mv ./hive ../hive_assets/

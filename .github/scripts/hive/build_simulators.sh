#!/usr/bin/env bash
set -eo pipefail

fixture_variant="${1:-amsterdam}"
# Cache-bump for the EELS frame-mapping update (a1945ddfd).

case "${fixture_variant}" in
    amsterdam)
        eels_fixtures="https://github.com/ethereum/execution-specs/releases/download/tests-glamsterdam-devnet@v7.2.1/fixtures_glamsterdam-devnet.tar.gz"
        eels_branch="devnets/glamsterdam/7"
        eels_fork="Amsterdam"
        ;;
    osaka)
        eels_fixtures="https://github.com/ethereum/execution-spec-tests/releases/download/v5.3.0/fixtures_develop.tar.gz"
        eels_branch="mainnet"
        eels_fork="Osaka"
        ;;
    bogota)
        eels_fixtures="https://github.com/ethereum/execution-specs/releases/download/tests-frames-devnet@v0.3.0/fixtures_frames-devnet.tar.gz"
        eels_branch="devnets/frames/0"
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

# Build the fixture-heavy EELS images serially to avoid exhausting the runner
# while both builds download and unpack the same large fixture archive.
echo "Building images"
./hive -client reth --sim "ethereum/eels/consume-engine" \
    --sim.buildarg fixtures="${eels_fixtures}" \
    --sim.buildarg branch="${eels_branch}" \
    --sim.timelimit 1s || true
./hive -client reth --sim "ethereum/eels/consume-rlp" \
    --sim.buildarg fixtures="${eels_fixtures}" \
    --sim.buildarg branch="${eels_branch}" \
    --sim.timelimit 1s || true
./hive -client reth --sim "ethereum/eels/execute-blobs" \
    --sim.buildarg branch="${eels_branch}" \
    --sim.buildarg fork="${eels_fork}" \
    --sim.timelimit 1s || true &
./hive -client reth --sim "ethereum/engine" -sim.timelimit 1s || true &
./hive -client reth --sim "devp2p" -sim.timelimit 1s || true &
./hive -client reth --sim "ethereum/rpc-compat" -sim.timelimit 1s || true &
./hive -client reth --sim "smoke/genesis" -sim.timelimit 1s || true &
./hive -client reth --sim "smoke/network" -sim.timelimit 1s || true &
./hive -client reth --sim "ethereum/sync" -sim.timelimit 1s || true &
wait

for image in \
    hive/simulators/ethereum/eels/consume-engine:latest \
    hive/simulators/ethereum/eels/consume-rlp:latest; do
    docker image inspect "${image}" >/dev/null
done

# Run docker save in parallel, wait and exit on error
echo "Saving images"
saving_pids=( )
docker save hive/hiveproxy:latest -o ../hive_assets/hiveproxy.tar & saving_pids+=( $! )
docker save hive/simulators/devp2p:latest -o ../hive_assets/devp2p.tar & saving_pids+=( $! )
docker save hive/simulators/ethereum/engine:latest -o ../hive_assets/engine.tar & saving_pids+=( $! )
docker save hive/simulators/ethereum/rpc-compat:latest -o ../hive_assets/rpc_compat.tar & saving_pids+=( $! )
docker save hive/simulators/ethereum/eels/consume-engine:latest -o ../hive_assets/eels_engine.tar & saving_pids+=( $! )
docker save hive/simulators/ethereum/eels/consume-rlp:latest -o ../hive_assets/eels_rlp.tar & saving_pids+=( $! )
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

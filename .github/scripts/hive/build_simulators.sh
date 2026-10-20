#!/usr/bin/env bash
set -eo pipefail

fixture_variant="${1:-amsterdam}"
# EELS simulator image tag, tests@v21.0.1 covers Osaka, BPO and Amsterdam
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
        # Bogota fixtures are not part of the mainnet simulator image release.
        eels_fixtures="https://github.com/ethereum/execution-specs/releases/download/tests-bogota-devnet@v1.1.0/fixtures_bogota-devnet.tar.gz"
        eels_branch="devnets/bogota/1"
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

# Build EELS images serially because Bogota still downloads and unpacks fixtures.
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

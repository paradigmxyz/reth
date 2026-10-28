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
        eels_fixtures="https://github.com/ethereum/execution-specs/releases/download/tests-bogota-devnet%40v2.0.0/fixtures_bogota-devnet.tar.gz"
        eels_branch="devnets/bogota/2"
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

# Reclaim intermediate build layers and stopped simulation containers before export.
docker container prune --force
docker builder prune --all --force
df -h .

# Each export needs temporary disk space in Docker. Save serially and compress the
# stream so fixture layers are never also stored as uncompressed archive files.
echo "Saving images"
save_image() {
    local image="$1"
    local archive="$2"
    echo "Saving ${image}"
    docker save "${image}" | gzip -1 > "../hive_assets/${archive}.tar.gz"
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
df -h .

# Make sure we don't rebuild images on the CI jobs
git apply ../.github/scripts/hive/no_sim_build.diff
go build .
mv ./hive ../hive_assets/

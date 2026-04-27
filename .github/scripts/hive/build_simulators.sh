#!/usr/bin/env bash
set -eo pipefail

fixture_variant="${1:-osaka}"

case "${fixture_variant}" in
    amsterdam)
        eels_fixtures="https://github.com/ethereum/execution-spec-tests/releases/download/bal@v5.7.0/fixtures_bal.tar.gz"
        eels_branch="devnets/bal/4"
        ;;
    osaka)
        eels_fixtures="https://github.com/ethereum/execution-spec-tests/releases/download/v5.3.0/fixtures_develop.tar.gz"
        eels_branch="forks/osaka"
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

# Run each hive command in the background for each simulator and wait
echo "Building images"
./hive -client reth --sim "ethereum/eels/consume-engine" \
    --sim.buildarg fixtures="${eels_fixtures}" \
    --sim.buildarg branch="${eels_branch}" \
    --sim.timelimit 1s || true &
./hive -client reth --sim "ethereum/eels/consume-rlp" \
    --sim.buildarg fixtures="${eels_fixtures}" \
    --sim.buildarg branch="${eels_branch}" \
    --sim.timelimit 1s || true &
./hive -client reth --sim "ethereum/engine" -sim.timelimit 1s || true &
./hive -client reth --sim "devp2p" -sim.timelimit 1s || true &
./hive -client reth --sim "ethereum/rpc-compat" -sim.timelimit 1s || true &
./hive -client reth --sim "smoke/genesis" -sim.timelimit 1s || true &
./hive -client reth --sim "smoke/network" -sim.timelimit 1s || true &
./hive -client reth --sim "ethereum/sync" -sim.timelimit 1s || true &
wait

# Run docker save sequentially
echo "Saving images"
docker save hive/hiveproxy:latest -o ../hive_assets/hiveproxy.tar
docker save hive/simulators/devp2p:latest -o ../hive_assets/devp2p.tar
docker save hive/simulators/ethereum/engine:latest -o ../hive_assets/engine.tar
docker save hive/simulators/ethereum/rpc-compat:latest -o ../hive_assets/rpc_compat.tar
docker save hive/simulators/ethereum/eels/consume-engine:latest -o ../hive_assets/eels_engine.tar
docker save hive/simulators/ethereum/eels/consume-rlp:latest -o ../hive_assets/eels_rlp.tar
docker save hive/simulators/smoke/genesis:latest -o ../hive_assets/smoke_genesis.tar
docker save hive/simulators/smoke/network:latest -o ../hive_assets/smoke_network.tar
docker save hive/simulators/ethereum/sync:latest -o ../hive_assets/ethereum_sync.tar

# Make sure we don't rebuild images on the CI jobs
git apply ../.github/scripts/hive/no_sim_build.diff
go build .
mv ./hive ../hive_assets/

#!/usr/bin/env bash
set -eo pipefail

simulator="${1:?missing simulator name}"

case "${simulator}" in
    ethereum/eels/consume-engine)
        simulator_image="/tmp/eels_engine.tar"
        ;;
    ethereum/eels/consume-rlp)
        simulator_image="/tmp/eels_rlp.tar"
        ;;
    ethereum/eels/execute-blobs)
        simulator_image="/tmp/eels_blobs.tar"
        ;;
    ethereum/engine)
        simulator_image="/tmp/engine.tar"
        ;;
    devp2p)
        simulator_image="/tmp/devp2p.tar"
        ;;
    ethereum/rpc-compat)
        simulator_image="/tmp/rpc_compat.tar"
        ;;
    smoke/genesis)
        simulator_image="/tmp/smoke_genesis.tar"
        ;;
    smoke/network)
        simulator_image="/tmp/smoke_network.tar"
        ;;
    ethereum/sync)
        simulator_image="/tmp/ethereum_sync.tar"
        ;;
    *)
        echo "unknown simulator: ${simulator}" >&2
        exit 1
        ;;
esac

# Only the selected simulator needs to be loaded. The EELS images contain the
# fixture cache and loading both of them can exhaust the runner's disk.
IMAGES=(
    "/tmp/hiveproxy.tar"
    "/tmp/reth_image.tar"
    "${simulator_image}"
)

# Load images serially so Docker does not unpack multiple large layers at once.
for IMAGE_TAR in "${IMAGES[@]}"; do
    echo "Loading image $IMAGE_TAR..."
    docker load -i "$IMAGE_TAR"
done

docker image ls -a

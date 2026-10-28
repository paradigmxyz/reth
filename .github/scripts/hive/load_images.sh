#!/usr/bin/env bash
set -eo pipefail

simulator="${1:?missing simulator name}"

case "${simulator}" in
    ethereum/eels/consume-engine)
        simulator_image="/tmp/eels_engine.tar.gz"
        ;;
    ethereum/eels/consume-rlp)
        simulator_image="/tmp/eels_rlp.tar.gz"
        ;;
    ethereum/eels/execute-blobs)
        simulator_image="/tmp/eels_blobs.tar.gz"
        ;;
    ethereum/engine)
        simulator_image="/tmp/engine.tar.gz"
        ;;
    devp2p)
        simulator_image="/tmp/devp2p.tar.gz"
        ;;
    ethereum/rpc-compat)
        simulator_image="/tmp/rpc_compat.tar.gz"
        ;;
    smoke/genesis)
        simulator_image="/tmp/smoke_genesis.tar.gz"
        ;;
    smoke/network)
        simulator_image="/tmp/smoke_network.tar.gz"
        ;;
    ethereum/sync)
        simulator_image="/tmp/ethereum_sync.tar.gz"
        ;;
    *)
        echo "unknown simulator: ${simulator}" >&2
        exit 1
        ;;
esac

# Only the selected simulator needs to be loaded. The EELS images contain the
# fixture cache and loading both of them can exhaust the runner's disk.
IMAGES=(
    "/tmp/hiveproxy.tar.gz"
    "/tmp/reth_image.tar"
    "${simulator_image}"
)

# Load images serially so Docker does not unpack multiple large layers at once.
for IMAGE_TAR in "${IMAGES[@]}"; do
    echo "Loading image $IMAGE_TAR..."
    docker load -i "$IMAGE_TAR"
done

docker image ls -a

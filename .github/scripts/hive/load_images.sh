#!/usr/bin/env bash
set -eo pipefail

# List of image archives to load.
IMAGES=(
    "/tmp/hiveproxy.tar.gz"
    "/tmp/devp2p.tar.gz"
    "/tmp/engine.tar.gz"
    "/tmp/rpc_compat.tar.gz"
    "/tmp/smoke_genesis.tar.gz"
    "/tmp/smoke_network.tar.gz"
    "/tmp/ethereum_sync.tar.gz"
    "/tmp/eels_engine.tar.gz"
    "/tmp/eels_rlp.tar.gz"
    "/tmp/eels_blobs.tar.gz"
    "/tmp/reth_image.tar"
)

# Load serially to limit peak disk usage and propagate failures.
for IMAGE_TAR in "${IMAGES[@]}"; do
    echo "Loading image $IMAGE_TAR..."
    docker load -i "$IMAGE_TAR"
done

docker image ls -a

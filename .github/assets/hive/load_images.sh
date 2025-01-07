#!/usr/bin/env bash
set -eo pipefail

# List of tar files to load
IMAGES=(
    "/tmp/hiveproxy.tar"
    "/tmp/devp2p.tar"
    "/tmp/engine.tar"
    "/tmp/rpc_compat.tar"
    "/tmp/pyspec.tar"
    "/tmp/smoke_genesis.tar"
    "/tmp/smoke_network.tar"
    "/tmp/ethereum_sync.tar"
    "/tmp/reth_image.tar"
)

# Loop through the images and load them
for IMAGE_TAR in "${IMAGES[@]}"; do
    echo "Loading image $IMAGE_TAR..."
    docker load -i "$IMAGE_TAR" &
done

wait

docker image ls -a
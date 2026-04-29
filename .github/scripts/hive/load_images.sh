#!/usr/bin/env bash
set -eo pipefail

# List of tar files to load
IMAGES=(
    "/tmp/hiveproxy.tar"
    "/tmp/devp2p.tar"
    "/tmp/engine.tar"
    "/tmp/rpc_compat.tar"
    "/tmp/smoke_genesis.tar"
    "/tmp/smoke_network.tar"
    "/tmp/ethereum_sync.tar"
    "/tmp/eels_engine.tar"
    "/tmp/eels_rlp.tar"
    "/tmp/reth_image.tar"
)

# Loop through the images and load them
load_pids=( )
for IMAGE_TAR in "${IMAGES[@]}"; do
    echo "Loading image $IMAGE_TAR..."
    docker load -i "$IMAGE_TAR" &
    load_pids+=( "$!" )
done

failed=0
for index in "${!load_pids[@]}"; do
    if ! wait "${load_pids[$index]}"; then
        echo "Failed to load image ${IMAGES[$index]}" >&2
        failed=1
    fi
done

if [ "$failed" -ne 0 ]; then
    exit 1
fi

docker image ls -a

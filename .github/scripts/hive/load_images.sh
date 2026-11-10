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
    "/tmp/eels_enginex.tar"
    "/tmp/eels_wirex.tar"
    "/tmp/eels_blobs.tar"
    "/tmp/reth_image.tar"
)

# Load every image and propagate failures from background jobs.
loading_pids=( )
for IMAGE_TAR in "${IMAGES[@]}"; do
    echo "Loading image $IMAGE_TAR..."
    docker load -i "$IMAGE_TAR" &
    loading_pids+=( $! )
done

for pid in "${loading_pids[@]}"; do
    wait "$pid" || exit
done

docker image ls -a

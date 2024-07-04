#!/usr/bin/env bash
set -eo pipefail

docker load --input /tmp/hiveproxy.tar & 
docker load --input /tmp/devp2p.tar &
docker load --input /tmp/engine.tar &
docker load --input /tmp/rpc_compat.tar &
docker load --input /tmp/pyspec.tar &
docker load --input /tmp/smoke_genesis.tar &
docker load --input /tmp/smoke_network.tar &
docker load --input /tmp/ethereum_sync.tar &
docker load --input /tmp/reth_image.tar &
wait

docker image ls -a
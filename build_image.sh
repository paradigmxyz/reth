#!/bin/bash

GITHASH=$(git rev-parse --short HEAD)
TAG="op-reth:$GITHASH"

echo "🔨 Building Reth Docker image: $TAG ..."

docker build -t $TAG -f DockerfileOp .

if [ $# -gt 0 ] && [ "$1" == "-l" ]; then
    docker tag $TAG op-reth:latest
    echo "🔖 Tagged $TAG as op-reth:latest"
else
    echo "💡 Use -l to tag the image as latest"
fi

#!/bin/bash

# Build script for rollkit-reth Docker image with optimized caching

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}Building rollkit-reth Docker image with optimized caching...${NC}"

# Enable BuildKit for better caching and parallel builds
export DOCKER_BUILDKIT=1

# Build arguments
BUILD_PROFILE=${BUILD_PROFILE:-release}
FEATURES=${FEATURES:-""}
IMAGE_TAG=${IMAGE_TAG:-rollkit-reth:latest}

echo -e "${YELLOW}Build configuration:${NC}"
echo "  Profile: $BUILD_PROFILE"
echo "  Features: $FEATURES"
echo "  Image tag: $IMAGE_TAG"
echo ""

# Build with BuildKit cache mounts and multi-platform support
docker build \
    --file Dockerfile.rollkit \
    --tag "$IMAGE_TAG" \
    --build-arg BUILD_PROFILE="$BUILD_PROFILE" \
    --build-arg FEATURES="$FEATURES" \
    --progress=plain \
    .

echo -e "${GREEN}✓ Build completed successfully!${NC}"
echo -e "${YELLOW}Image size:${NC}"
docker images "$IMAGE_TAG" --format "table {{.Repository}}\t{{.Tag}}\t{{.Size}}"

echo ""
echo -e "${GREEN}To run the container:${NC}"
echo "  docker run --rm -p 8545:8545 -p 8551:8551 $IMAGE_TAG --help"
echo ""
echo -e "${GREEN}To use with docker-compose:${NC}"
echo "  docker-compose -f rollkit-reth-docker-compose.yml up -d" 
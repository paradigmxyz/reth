#!/usr/bin/env bash
# Verifies that Docker images have the expected architectures.
#
# Usage:
#   ./verify_image_arch.sh <targets> <registry> <ethereum_tags>
#
# Environment:
#   DRY_RUN=true  - Skip actual verification, just print what would be checked.

set -euo pipefail

TARGETS="${1:-}"
REGISTRY="${2:-}"
ETHEREUM_TAGS="${3:-}"
DRY_RUN="${DRY_RUN:-false}"

verify_image() {
    local image="$1"
    shift
    local expected_archs=("$@")

    echo "Checking $image..."

    if [[ "$DRY_RUN" == "true" ]]; then
        echo "  [dry-run] Would verify architectures: ${expected_archs[*]}"
        return 0
    fi

    manifest=$(docker manifest inspect "$image" 2>/dev/null) || {
        echo "::error::Failed to inspect manifest for $image"
        return 1
    }

    for arch in "${expected_archs[@]}"; do
        if ! echo "$manifest" | jq -e ".manifests[] | select(.platform.architecture == \"$arch\" and .platform.os == \"linux\")" > /dev/null; then
            echo "::error::Missing architecture $arch for $image"
            return 1
        fi
        echo "  ✓ linux/$arch"
    done
}

if [[ "$TARGETS" == *"nightly"* ]]; then
    verify_image "${REGISTRY}/reth:nightly" amd64 arm64
    verify_image "${REGISTRY}/reth:nightly-profiling" amd64
    verify_image "${REGISTRY}/reth:nightly-edge-profiling" amd64
else
    for tag in $(echo "$ETHEREUM_TAGS" | tr ',' ' '); do
        verify_image "$tag" amd64 arm64
    done
fi

echo "All image architectures verified successfully"

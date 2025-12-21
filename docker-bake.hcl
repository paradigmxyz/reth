// Docker Bake configuration for reth and op-reth images
// Usage:
//   docker buildx bake ethereum    # Build reth
//   docker buildx bake optimism    # Build op-reth
//   docker buildx bake             # Build all

variable "REGISTRY" {
  default = "ghcr.io/paradigmxyz"
}

variable "TAG" {
  default = "latest"
}

variable "BUILD_PROFILE" {
  default = "maxperf"
}

variable "FEATURES" {
  default = "jemalloc asm-keccak"
}

// Common settings for all targets
group "default" {
  targets = ["ethereum", "optimism"]
}

group "nightly" {
  targets = ["ethereum", "ethereum-profiling", "optimism", "optimism-profiling"]
}

// Base target with shared configuration
target "_base" {
  dockerfile = "Dockerfile.depot"
  platforms  = ["linux/amd64", "linux/arm64"]
  args = {
    BUILD_PROFILE = "${BUILD_PROFILE}"
    FEATURES      = "${FEATURES}"
  }
}

// Ethereum (reth)
target "ethereum" {
  inherits = ["_base"]
  args = {
    BINARY        = "reth"
    MANIFEST_PATH = "bin/reth"
  }
  tags = ["${REGISTRY}/reth:${TAG}"]
}

target "ethereum-profiling" {
  inherits = ["_base"]
  args = {
    BINARY        = "reth"
    MANIFEST_PATH = "bin/reth"
    BUILD_PROFILE = "profiling"
    FEATURES      = "jemalloc asm-keccak jemalloc-prof"
  }
  tags = ["${REGISTRY}/reth:nightly-profiling"]
}

// Optimism (op-reth)
target "optimism" {
  inherits = ["_base"]
  args = {
    BINARY        = "op-reth"
    MANIFEST_PATH = "crates/optimism/bin"
  }
  tags = ["${REGISTRY}/op-reth:${TAG}"]
}

target "optimism-profiling" {
  inherits = ["_base"]
  args = {
    BINARY        = "op-reth"
    MANIFEST_PATH = "crates/optimism/bin"
    BUILD_PROFILE = "profiling"
    FEATURES      = "jemalloc asm-keccak jemalloc-prof"
  }
  tags = ["${REGISTRY}/op-reth:nightly-profiling"]
}

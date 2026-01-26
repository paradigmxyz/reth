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
  default = "jemalloc asm-keccak min-debug-logs"
}

// Git info for vergen (since .git is excluded from Docker context)
variable "VERGEN_GIT_SHA" {
  default = ""
}

variable "VERGEN_GIT_DESCRIBE" {
  default = ""
}

variable "VERGEN_GIT_DIRTY" {
  default = "false"
}

// Common settings for all targets
group "default" {
  targets = ["ethereum", "optimism"]
}

group "nightly" {
  targets = ["ethereum", "ethereum-profiling", "ethereum-edge-profiling", "optimism", "optimism-profiling", "optimism-edge-profiling"]
}

// Base target with shared configuration
target "_base" {
  dockerfile = "Dockerfile.depot"
  platforms  = ["linux/amd64", "linux/arm64"]
  args = {
    BUILD_PROFILE      = "${BUILD_PROFILE}"
    FEATURES           = "${FEATURES}"
    VERGEN_GIT_SHA     = "${VERGEN_GIT_SHA}"
    VERGEN_GIT_DESCRIBE = "${VERGEN_GIT_DESCRIBE}"
    VERGEN_GIT_DIRTY   = "${VERGEN_GIT_DIRTY}"
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
    FEATURES      = "jemalloc jemalloc-prof asm-keccak min-debug-logs"
  }
  tags = ["${REGISTRY}/reth:nightly-profiling"]
}

target "ethereum-edge-profiling" {
  inherits = ["_base"]
  args = {
    BINARY        = "reth"
    MANIFEST_PATH = "bin/reth"
    BUILD_PROFILE = "profiling"
    FEATURES      = "jemalloc jemalloc-prof asm-keccak min-debug-logs edge"
  }
  tags = ["${REGISTRY}/reth:nightly-edge-profiling"]
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
    FEATURES      = "jemalloc jemalloc-prof asm-keccak min-debug-logs"
  }
  tags = ["${REGISTRY}/op-reth:nightly-profiling"]
}

target "optimism-edge-profiling" {
  inherits = ["_base"]
  args = {
    BINARY        = "op-reth"
    MANIFEST_PATH = "crates/optimism/bin"
    BUILD_PROFILE = "profiling"
    FEATURES      = "jemalloc jemalloc-prof asm-keccak min-debug-logs edge"
  }
  tags = ["${REGISTRY}/op-reth:nightly-edge-profiling"]
}

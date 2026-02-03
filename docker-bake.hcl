// Docker Bake configuration for reth images

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
  targets = [
    "ethereum-amd64",
    "ethereum-arm64",
    "optimism-amd64",
    "optimism-arm64"
  ]
}

group "nightly" {
  targets = [
    "ethereum-amd64",
    "ethereum-arm64",
    "ethereum-profiling",
    "ethereum-edge-profiling",
    "optimism-amd64",
    "optimism-arm64",
    "optimism-profiling"
  ]
}

// Base target with shared configuration
target "_base" {
  dockerfile = "Dockerfile.depot"
  args = {
    BUILD_PROFILE       = "${BUILD_PROFILE}"
    FEATURES            = "${FEATURES}"
    VERGEN_GIT_SHA      = "${VERGEN_GIT_SHA}"
    VERGEN_GIT_DESCRIBE = "${VERGEN_GIT_DESCRIBE}"
    VERGEN_GIT_DIRTY    = "${VERGEN_GIT_DIRTY}"
  }
  secret = [
    {
      type = "env"
      id   = "DEPOT_TOKEN"
    }
  ]
}

// amd64 base with x86-64-v3 optimizations
target "_base_amd64" {
  inherits  = ["_base"]
  platforms = ["linux/amd64"]
  args = {
    # pclmulqdq required for rocksdb: https://github.com/rust-rocksdb/rust-rocksdb/issues/1069
    RUSTFLAGS = "-C target-cpu=x86-64-v3 -C target-feature=+pclmulqdq"
  }
}

// arm64 base
target "_base_arm64" {
  inherits  = ["_base"]
  platforms = ["linux/arm64"]
}

target "_base_profiling" {
  inherits  = ["_base_amd64"]
}

// Ethereum (reth)
target "ethereum-amd64" {
  inherits = ["_base_amd64"]
  args = {
    BINARY        = "reth"
    MANIFEST_PATH = "bin/reth"
  }
  tags = ["${REGISTRY}/reth:${TAG}"]
}

target "ethereum-arm64" {
  inherits = ["_base_arm64"]
  args = {
    BINARY        = "reth"
    MANIFEST_PATH = "bin/reth"
  }
  tags = ["${REGISTRY}/reth:${TAG}"]
}

target "ethereum-profiling" {
  inherits = ["_base_profiling"]
  args = {
    BINARY        = "reth"
    MANIFEST_PATH = "bin/reth"
    BUILD_PROFILE = "profiling"
    FEATURES      = "jemalloc jemalloc-prof asm-keccak min-debug-logs"
  }
  tags = ["${REGISTRY}/reth:nightly-profiling"]
}

target "ethereum-edge-profiling" {
  inherits = ["_base_profiling"]
  args = {
    BINARY        = "reth"
    MANIFEST_PATH = "bin/reth"
    BUILD_PROFILE = "profiling"
    FEATURES      = "jemalloc jemalloc-prof asm-keccak min-debug-logs edge"
  }
  tags = ["${REGISTRY}/reth:nightly-edge-profiling"]
}

// Optimism (op-reth)
target "optimism-amd64" {
  inherits = ["_base_amd64"]
  args = {
    BINARY        = "op-reth"
    MANIFEST_PATH = "crates/optimism/bin"
  }
  tags = ["${REGISTRY}/op-reth:${TAG}"]
}

target "optimism-arm64" {
  inherits = ["_base_arm64"]
  args = {
    BINARY        = "op-reth"
    MANIFEST_PATH = "crates/optimism/bin"
  }
  tags = ["${REGISTRY}/op-reth:${TAG}"]
}

target "optimism-profiling" {
  inherits = ["_base_profiling"]
  args = {
    BINARY        = "op-reth"
    MANIFEST_PATH = "crates/optimism/bin"
    BUILD_PROFILE = "profiling"
    FEATURES      = "jemalloc jemalloc-prof asm-keccak min-debug-logs"
  }
  tags = ["${REGISTRY}/op-reth:nightly-profiling"]
}

target "optimism-edge-profiling" {
  inherits = ["_base_profiling"]
  args = {
    BINARY        = "op-reth"
    MANIFEST_PATH = "crates/optimism/bin"
    BUILD_PROFILE = "profiling"
    FEATURES      = "jemalloc jemalloc-prof asm-keccak min-debug-logs edge"
  }
  tags = ["${REGISTRY}/op-reth:nightly-edge-profiling"]
}

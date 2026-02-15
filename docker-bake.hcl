// Docker Bake configuration for reth images

variable "REGISTRY" {
  default = "ghcr.io/paradigmxyz"
}

variable "TAG" {
  default = "latest"
}

variable "BUILD_PROFILE" {
  default = "maxperf-symbols"
}

variable "FEATURES" {
  default = ""
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
  targets = ["ethereum"]
}

group "nightly" {
  targets = ["ethereum", "ethereum-profiling", "ethereum-edge-profiling"]
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
  secret = [
    {
      type = "env"
      id   = "DEPOT_TOKEN"
    }
  ]
}
target "_base_profiling" {
  inherits = ["_base"]
  platforms  = ["linux/amd64"]
}
target "_base_profiling" {
  inherits = ["_base"]
  platforms  = ["linux/amd64"]
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
  inherits = ["_base_profiling"]
  args = {
    BINARY        = "reth"
    MANIFEST_PATH = "bin/reth"
    BUILD_PROFILE = "profiling"
    FEATURES      = "jemalloc-prof"
  }
  tags = ["${REGISTRY}/reth:nightly-profiling"]
}

target "ethereum-edge-profiling" {
  inherits = ["_base_profiling"]
  args = {
    BINARY        = "reth"
    MANIFEST_PATH = "bin/reth"
    BUILD_PROFILE = "profiling"
    FEATURES      = "jemalloc-prof edge"
  }
  tags = ["${REGISTRY}/reth:nightly-edge-profiling"]
}

// Hive test targets — single-platform, hivetests profile, tar output
target "_base_hive" {
  inherits  = ["_base"]
  platforms = ["linux/amd64"]
  args = {
    BUILD_PROFILE = "hivetests"
  }
}

variable "HIVE_OUTPUT_DIR" {
  default = "./artifacts"
}

target "hive-stable" {
  inherits = ["_base_hive"]
  args = {
    BINARY        = "reth"
    MANIFEST_PATH = "bin/reth"
  }
  tags   = ["reth:local"]
  output = ["type=docker,dest=${HIVE_OUTPUT_DIR}/reth_image.tar"]
}

target "hive-edge" {
  inherits = ["_base_hive"]
  args = {
    BINARY        = "reth"
    MANIFEST_PATH = "bin/reth"
    FEATURES      = "edge"
  }
  tags   = ["reth:local"]
  output = ["type=docker,dest=${HIVE_OUTPUT_DIR}/reth_image.tar"]
}

// Kurtosis test target
target "kurtosis" {
  inherits  = ["_base_hive"]
  args = {
    BINARY        = "reth"
    MANIFEST_PATH = "bin/reth"
  }
  tags   = ["ghcr.io/paradigmxyz/reth:kurtosis-ci"]
  output = ["type=docker,dest=${HIVE_OUTPUT_DIR}/reth_image.tar"]
}

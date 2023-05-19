# Heavily inspired by Lighthouse: https://github.com/sigp/lighthouse/blob/693886b94176faa4cb450f024696cb69cda2fe58/Makefile

GIT_TAG := $(shell git describe --tags --candidates 1)
BIN_DIR = "bin"

X86_64_TAG = "x86_64-unknown-linux-gnu"
BUILD_PATH_X86_64 = "target/$(X86_64_TAG)/release"
AARCH64_TAG = "aarch64-unknown-linux-gnu"
BUILD_PATH_AARCH64 = "target/$(AARCH64_TAG)/release"

# List of features to use when building natively. Can be overriden via the environment.
# No jemalloc on Windows
ifeq ($(OS),Windows_NT)
    FEATURES?=
else
    FEATURES?=jemalloc
endif

# List of features to use when cross-compiling. Can be overridden via the environment.
CROSS_FEATURES ?= jemalloc

# Cargo profile for Cross builds. Default is for local builds, CI uses an override.
CROSS_PROFILE ?= release

# Cargo profile for regular builds.
PROFILE ?= release

# Extra flags for Cargo
CARGO_INSTALL_EXTRA_FLAGS ?=

# Builds the reth binary in release (optimized).
#
# Binaries will most likely be found in `./target/release`
install:
	cargo install --path reth --force --locked \
		--features "$(FEATURES)" \
		--profile "$(PROFILE)" \
		$(CARGO_INSTALL_EXTRA_FLAGS)

# The following commands use `cross` to build a cross-compile.
#
# These commands require that:
#
# - `cross` is installed (`cargo install cross`).
# - Docker is running.
# - The current user is in the `docker` group.
#
# The resulting binaries will be created in the `target/` directory.
#
# Note: The additional rustc compiler flags are for intrinsics needed by MDBX.
# See: https://github.com/cross-rs/cross/wiki/FAQ#undefined-reference-with-build-std
build-x86_64 build-aarch64: export RUSTFLAGS=-C link-arg=-lgcc -Clink-arg=-static-libgcc

build-x86_64:
	cross build --bin reth --target x86_64-unknown-linux-gnu --features "$(CROSS_FEATURES)" --profile "$(CROSS_PROFILE)"
build-aarch64:
	cross build --bin reth --target aarch64-unknown-linux-gnu --features "$(CROSS_FEATURES)" --profile "$(CROSS_PROFILE)"

# Create a `.tar.gz` containing a binary for a specific target.
define tarball_release_binary
	cp $(1)/reth $(BIN_DIR)/reth
	cd $(BIN_DIR) && \
		tar -czf reth-$(GIT_TAG)-$(2)$(3).tar.gz reth && \
		rm reth
endef

# Create a series of `.tar.gz` files in the BIN_DIR directory, each containing
# a `reth` binary for a different target.
#
# The current git tag will be used as the version in the output file names. You
# will likely need to use `git tag` and create a semver tag (e.g., `v0.2.3`).
build-release-tarballs:
	[ -d $(BIN_DIR) ] || mkdir -p $(BIN_DIR)
	$(MAKE) build-x86_64
	$(call tarball_release_binary,$(BUILD_PATH_X86_64),$(X86_64_TAG),"")
	$(MAKE) build-aarch64
	$(call tarball_release_binary,$(BUILD_PATH_AARCH64),$(AARCH64_TAG),"")

# Performs a `cargo` clean
clean:
	cargo clean
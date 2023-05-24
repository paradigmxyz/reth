# Heavily inspired by Lighthouse: https://github.com/sigp/lighthouse/blob/693886b94176faa4cb450f024696cb69cda2fe58/Makefile

GIT_TAG := $(shell git describe --tags --candidates 1)
BIN_DIR = "dist/bin"

BUILD_PATH = "target"

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
	cargo install --path bin/reth --bin reth --force --locked \
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
build-% build-x86_64-windows-gnu: export RUSTFLAGS=-C link-arg=-lgcc -Clink-arg=-static-libgcc

build-%:
	cross build --bin reth --target $* --features "$(CROSS_FEATURES)" --profile "$(CROSS_PROFILE)"
build-x86_64-pc-windows-gnu:
	# No jemalloc on Windows
	$(eval CROSS_FEATURES := $(filter-out jemalloc,$(CROSS_FEATURES)))
	cross build --bin reth --target x86_64-pc-windows-gnu --features "$(CROSS_FEATURES)" --profile "$(CROSS_PROFILE)"

# Create a `.tar.gz` containing a binary for a specific target.
define tarball_release_binary
	cp $(BUILD_PATH)/$(1)/$(CROSS_PROFILE)/$(2) $(BIN_DIR)/$(2)
	cd $(BIN_DIR) && \
		tar -czf reth-$(GIT_TAG)-$(1)$(3).tar.gz $(2) && \
		rm $(2)
endef

# Create a series of `.tar.gz` files in the BIN_DIR directory, each containing
# a `reth` binary for a different target.
#
# The current git tag will be used as the version in the output file names. You
# will likely need to use `git tag` and create a semver tag (e.g., `v0.2.3`).
build-release-tarballs:
	[ -d $(BIN_DIR) ] || mkdir -p $(BIN_DIR)
	$(MAKE) build-x86_64-unknown-linux-gnu
	$(call tarball_release_binary,"x86_64-unknown-linux-gnu","reth","")
	$(MAKE) build-aarch64-unknown-linux-gnu
	$(call tarball_release_binary,"aarch64-unknown-linux-gnu","reth","")
	$(MAKE) build-x86_64-pc-windows-gnu
	$(call tarball_release_binary,"x86_64-pc-windows-gnu","reth.exe","")

# Performs a `cargo` clean and removes the `dist` directory
clean:
	cargo clean
	rm -r dist
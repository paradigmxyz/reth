# Heavily inspired by Lighthouse: https://github.com/sigp/lighthouse/blob/693886b94176faa4cb450f024696cb69cda2fe58/Makefile

GIT_TAG := $(shell git describe --tags --candidates 1)
BIN_DIR = "dist/bin"

BUILD_PATH = "target"

# List of features to use when building. Can be overriden via the environment.
# No jemalloc on Windows
ifeq ($(OS),Windows_NT)
    FEATURES ?=
else
    FEATURES ?= jemalloc
endif

# Cargo profile for builds. Default is for local builds, CI uses an override.
PROFILE ?= release

# Extra flags for Cargo
CARGO_INSTALL_EXTRA_FLAGS ?=

# Builds and installs the reth binary.
#
# The binary will most likely be in `~/.cargo/bin`
install:
	cargo install --path bin/reth --bin reth --force --locked \
		--features "$(FEATURES)" \
		--profile "$(PROFILE)" \
		$(CARGO_INSTALL_EXTRA_FLAGS)

# Builds the reth binary natively.
build-native-%:
	cargo build --bin reth --target $* --features "$(FEATURES)" --profile "$(PROFILE)"

# The following commands use `cross` to build a cross-compile.
#
# These commands require that:
#
# - `cross` is installed (`cargo install cross`).
# - Docker is running.
# - The current user is in the `docker` group.
#
# The resulting binaries will be created in the `target/` directory.

# No jemalloc on Windows
build-x86_64-pc-windows-gnu: FEATURES := $(filter-out jemalloc,$(FEATURES))

# Note: The additional rustc compiler flags are for intrinsics needed by MDBX.
# See: https://github.com/cross-rs/cross/wiki/FAQ#undefined-reference-with-build-std
build-%:
	RUSTFLAGS="-C link-arg=-lgcc -Clink-arg=-static-libgcc" \
 		cross build --bin reth --target $* --features "$(FEATURES)" --profile "$(PROFILE)"

# Unfortunately we can't easily use cross to build for Darwin because of licensing issues.
# If we wanted to, we would need to build a custom Docker image with the SDK available.
#
# Note: You must set `SDKROOT` and `MACOSX_DEPLOYMENT_TARGET`. These can be found using `xcrun`.
#
# `SDKROOT=$(xcrun -sdk macosx --show-sdk-path) MACOSX_DEPLOYMENT_TARGET=$(xcrun -sdk macosx --show-sdk-platform-version)`
build-x86_64-apple-darwin:
	$(MAKE) build-native-x86_64-apple-darwin
build-aarch64-apple-darwin:
	$(MAKE) build-native-aarch64-apple-darwin

# Create a `.tar.gz` containing a binary for a specific target.
define tarball_release_binary
	cp $(BUILD_PATH)/$(1)/$(PROFILE)/$(2) $(BIN_DIR)/$(2)
	cd $(BIN_DIR) && \
		tar -czf reth-$(GIT_TAG)-$(1)$(3).tar.gz $(2) && \
		rm $(2)
endef

# Create a series of `.tar.gz` files in the BIN_DIR directory, each containing
# a `reth` binary for a different target.
#
# The current git tag will be used as the version in the output file names. You
# will likely need to use `git tag` and create a semver tag (e.g., `v0.2.3`).
#
# Note: This excludes macOS tarballs because of SDK licensing issues.
build-release-tarballs:
	[ -d $(BIN_DIR) ] || mkdir -p $(BIN_DIR)
	$(MAKE) build-x86_64-unknown-linux-gnu
	$(call tarball_release_binary,"x86_64-unknown-linux-gnu","reth","")
	$(MAKE) build-aarch64-unknown-linux-gnu
	$(call tarball_release_binary,"aarch64-unknown-linux-gnu","reth","")
	$(MAKE) build-x86_64-pc-windows-gnu
	$(call tarball_release_binary,"x86_64-pc-windows-gnu","reth.exe","")

# Performs a `cargo` clean and removes the binary directory
clean:
	cargo clean
	rm -r $(BIN_DIR)
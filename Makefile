# Heavily inspired by Lighthouse: https://github.com/sigp/lighthouse/blob/693886b94176faa4cb450f024696cb69cda2fe58/Makefile
.DEFAULT_GOAL := help

GIT_TAG ?= $(shell git describe --tags --abbrev=0)
BIN_DIR = "dist/bin"

MDBX_PATH = "crates/storage/libmdbx-rs/mdbx-sys/libmdbx"
DB_TOOLS_DIR = "db-tools"
FULL_DB_TOOLS_DIR := $(shell pwd)/$(DB_TOOLS_DIR)/

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

# The release tag of https://github.com/ethereum/tests to use for EF tests
EF_TESTS_TAG := v12.2
EF_TESTS_URL := https://github.com/ethereum/tests/archive/refs/tags/$(EF_TESTS_TAG).tar.gz
EF_TESTS_DIR := ./testing/ef-tests/ethereum-tests

# The docker image name
DOCKER_IMAGE_NAME ?= ghcr.io/paradigmxyz/reth

##@ Help

.PHONY: help
help: ## Display this help.
	@awk 'BEGIN {FS = ":.*##"; printf "Usage:\n  make \033[36m<target>\033[0m\n"} /^[a-zA-Z_0-9-]+:.*?##/ { printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2 } /^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) } ' $(MAKEFILE_LIST)

##@ Build

.PHONY: install
install: ## Build and install the reth binary under `~/.cargo/bin`.
	cargo install --path bin/reth --bin reth --force --locked \
		--features "$(FEATURES)" \
		--profile "$(PROFILE)" \
		$(CARGO_INSTALL_EXTRA_FLAGS)

.PHONY: install-op
install-op: ## Build and install the op-reth binary under `~/.cargo/bin`.
	cargo install --path bin/reth --bin op-reth --force --locked \
		--features "optimism,$(FEATURES)" \
		--profile "$(PROFILE)" \
		$(CARGO_INSTALL_EXTRA_FLAGS)

# Builds the reth binary natively.
build-native-%:
	cargo build --bin reth --target $* --features "$(FEATURES)" --profile "$(PROFILE)"

op-build-native-%:
	cargo build --bin op-reth --target $* --features "optimism,$(FEATURES)" --profile "$(PROFILE)"

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
build-x86_64-pc-windows-gnu: FEATURES := $(filter-out jemalloc jemalloc-prof,$(FEATURES))

# Note: The additional rustc compiler flags are for intrinsics needed by MDBX.
# See: https://github.com/cross-rs/cross/wiki/FAQ#undefined-reference-with-build-std
build-%:
	RUSTFLAGS="-C link-arg=-lgcc -Clink-arg=-static-libgcc" \
 		cross build --bin reth --target $* --features "$(FEATURES)" --profile "$(PROFILE)"

op-build-%:
	RUSTFLAGS="-C link-arg=-lgcc -Clink-arg=-static-libgcc" \
 		cross build --bin op-reth --target $* --features "optimism,$(FEATURES)" --profile "$(PROFILE)"

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

# The current git tag will be used as the version in the output file names. You
# will likely need to use `git tag` and create a semver tag (e.g., `v0.2.3`).
#
# Note: This excludes macOS tarballs because of SDK licensing issues.
.PHONY: build-release-tarballs
build-release-tarballs: ## Create a series of `.tar.gz` files in the BIN_DIR directory, each containing a `reth` binary for a different target.
	[ -d $(BIN_DIR) ] || mkdir -p $(BIN_DIR)
	$(MAKE) build-x86_64-unknown-linux-gnu
	$(call tarball_release_binary,"x86_64-unknown-linux-gnu","reth","")
	$(MAKE) build-aarch64-unknown-linux-gnu
	$(call tarball_release_binary,"aarch64-unknown-linux-gnu","reth","")
	$(MAKE) build-x86_64-pc-windows-gnu
	$(call tarball_release_binary,"x86_64-pc-windows-gnu","reth.exe","")

##@ Test

UNIT_TEST_ARGS := --locked --workspace --features 'jemalloc-prof' -E 'kind(lib)' -E 'kind(bin)' -E 'kind(proc-macro)'
UNIT_TEST_ARGS_OP := --locked --workspace --features 'jemalloc-prof,optimism' -E 'kind(lib)' -E 'kind(bin)' -E 'kind(proc-macro)'
COV_FILE := lcov.info

.PHONY: test-unit
test-unit: ## Run unit tests.
	cargo install cargo-nextest --locked
	cargo nextest run $(UNIT_TEST_ARGS)

.PHONY: test-unit-op
test-unit-op: ## Run unit tests (with optimism feature flag enabled).
	cargo install cargo-nextest --locked
	cargo nextest run $(UNIT_TEST_ARGS_OP)

.PHONY: cov-unit
cov-unit: ## Run unit tests with coverage.
	rm -f $(COV_FILE)
	cargo llvm-cov nextest --lcov --output-path $(COV_FILE) $(UNIT_TEST_ARGS)

.PHONY: cov-unit-op
cov-unit-op: ## Run unit tests with coverage (with optimism feature flag enabled).
	rm -f $(COV_FILE)
	cargo llvm-cov nextest --lcov --output-path $(COV_FILE) $(UNIT_TEST_ARGS_OP)

.PHONY: cov-report-html
cov-report-html: cov-unit ## Generate a HTML coverage report and open it in the browser.
	cargo llvm-cov report --html
	open target/llvm-cov/html/index.html

# Downloads and unpacks Ethereum Foundation tests in the `$(EF_TESTS_DIR)` directory.
#
# Requires `wget` and `tar`
$(EF_TESTS_DIR):
	mkdir $(EF_TESTS_DIR)
	wget $(EF_TESTS_URL) -O ethereum-tests.tar.gz
	tar -xzf ethereum-tests.tar.gz --strip-components=1 -C $(EF_TESTS_DIR)
	rm ethereum-tests.tar.gz

.PHONY: ef-tests
ef-tests: $(EF_TESTS_DIR) ## Runs Ethereum Foundation tests.
	cargo nextest run -p ef-tests --features ef-tests

##@ Docker

# Note: This requires a buildx builder with emulation support. For example:
#
# `docker run --privileged --rm tonistiigi/binfmt --install amd64,arm64`
# `docker buildx create --use --driver docker-container --name cross-builder`
.PHONY: docker-build-latest
docker-build-latest: ## Build and push a cross-arch Docker image tagged with the latest git tag and `latest`.
	$(call build_docker_image,$(GIT_TAG),latest)

# Note: This requires a buildx builder with emulation support. For example:
#
# `docker run --privileged --rm tonistiigi/binfmt --install amd64,arm64`
# `docker buildx create --use --name cross-builder`
.PHONY: docker-build-nightly
docker-build-nightly: ## Build and push cross-arch Docker image tagged with the latest git tag with a `-nightly` suffix, and `latest-nightly`.
	$(call build_docker_image,$(GIT_TAG)-nightly,latest-nightly)

# Create a cross-arch Docker image with the given tags and push it
define build_docker_image
	$(MAKE) build-x86_64-unknown-linux-gnu
	mkdir -p $(BIN_DIR)/amd64
	cp $(BUILD_PATH)/x86_64-unknown-linux-gnu/$(PROFILE)/reth $(BIN_DIR)/amd64/reth

	$(MAKE) build-aarch64-unknown-linux-gnu
	mkdir -p $(BIN_DIR)/arm64
	cp $(BUILD_PATH)/aarch64-unknown-linux-gnu/$(PROFILE)/reth $(BIN_DIR)/arm64/reth

	docker buildx build --file ./Dockerfile.cross . \
		--platform linux/amd64,linux/arm64 \
		--tag $(DOCKER_IMAGE_NAME):$(1) \
		--tag $(DOCKER_IMAGE_NAME):$(2) \
		--provenance=false \
		--push
endef

##@ Other

.PHONY: clean
clean: ## Perform a `cargo` clean and remove the binary and test vectors directories.
	cargo clean
	rm -rf $(BIN_DIR)
	rm -rf $(EF_TESTS_DIR)

.PHONY: db-tools
db-tools: ## Compile MDBX debugging tools.
	@echo "Building MDBX debugging tools..."
    # `IOARENA=1` silences benchmarking info message that is printed to stderr
	@$(MAKE) -C $(MDBX_PATH) IOARENA=1 tools > /dev/null
	@mkdir -p $(DB_TOOLS_DIR)
	@cd $(MDBX_PATH) && \
		mv mdbx_chk $(FULL_DB_TOOLS_DIR) && \
		mv mdbx_copy $(FULL_DB_TOOLS_DIR) && \
		mv mdbx_dump $(FULL_DB_TOOLS_DIR) && \
		mv mdbx_drop $(FULL_DB_TOOLS_DIR) && \
		mv mdbx_load $(FULL_DB_TOOLS_DIR) && \
		mv mdbx_stat $(FULL_DB_TOOLS_DIR)
    # `IOARENA=1` silences benchmarking info message that is printed to stderr
	@$(MAKE) -C $(MDBX_PATH) IOARENA=1 clean > /dev/null
	@echo "Run \"$(DB_TOOLS_DIR)/mdbx_stat\" for the info about MDBX db file."
	@echo "Run \"$(DB_TOOLS_DIR)/mdbx_chk\" for the MDBX db file integrity check."

.PHONY: update-book-cli
update-book-cli: ## Update book cli documentation.
	cargo build --bin reth --features "$(FEATURES)" --profile "$(PROFILE)"
	@echo "Updating book cli doc..."
	@./book/cli/update.sh $(BUILD_PATH)

.PHONY: maxperf
maxperf:
	RUSTFLAGS="-C target-cpu=native" cargo build --profile maxperf --features jemalloc

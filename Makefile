# Heavily inspired by Lighthouse: https://github.com/sigp/lighthouse/blob/693886b94176faa4cb450f024696cb69cda2fe58/Makefile
.DEFAULT_GOAL := help

GIT_SHA ?= $(shell git rev-parse HEAD)
GIT_TAG ?= $(shell git describe --tags --abbrev=0)
BIN_DIR = "dist/bin"

MDBX_PATH = "crates/storage/libmdbx-rs/mdbx-sys/libmdbx"
DB_TOOLS_DIR = "db-tools"
FULL_DB_TOOLS_DIR := $(shell pwd)/$(DB_TOOLS_DIR)/

CARGO_TARGET_DIR ?= target

# List of features to use when building. Can be overridden via the environment.
# No jemalloc on Windows
ifeq ($(OS),Windows_NT)
    FEATURES ?= asm-keccak min-debug-logs
else
    FEATURES ?= jemalloc asm-keccak min-debug-logs
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

# Dependency check macros
define check_cmd
	@if ! command -v $(1) &> /dev/null; then \
		echo "\033[31mERROR: Command '$(1)' not found.\033[0m"; \
		echo "To install $(1): $(2)"; \
		exit 1; \
	fi
endef

define check_file_dir
	@if [ ! -d $(1) ]; then \
		echo "\033[31mERROR: Required directory '$(1)' not found.\033[0m"; \
		echo "$(2)"; \
		exit 1; \
	fi
endef

# Enhanced dependency check - tries to provide useful diagnostics
define check_cmd_diagnostic
	@if ! command -v $(1) &> /dev/null; then \
		echo "\033[31m========== ERROR: MISSING DEPENDENCY ==========\033[0m"; \
		echo "\033[31mRequired command '$(1)' not found in your PATH.\033[0m"; \
		echo ""; \
		echo "How to install $(1):"; \
		echo "$(2)"; \
		echo ""; \
		echo "After installing, run your command again."; \
		echo "\033[31m===============================================\033[0m"; \
		exit 1; \
	fi
endef

# Enhanced directory check - provides diagnostics on why a directory might be missing
define check_dir_diagnostic
	@if [ ! -d $(1) ]; then \
		echo "\033[31m============= ERROR: MISSING DIRECTORY =============\033[0m"; \
		echo "\033[31mDirectory '$(1)' not found.\033[0m"; \
		echo ""; \
		echo "This might be because:"; \
		echo "1. You haven't downloaded the required files"; \
		echo "2. The download failed or was interrupted"; \
		echo "3. The files were moved or deleted"; \
		echo ""; \
		echo "How to fix:"; \
		echo "$(2)"; \
		echo ""; \
		echo "\033[31m====================================================\033[0m"; \
		exit 1; \
	fi
endef

# Dependency check functions
.PHONY: ensure-wget
ensure-wget:
	@if ! command -v wget &> /dev/null; then \
		echo "\033[31m========== ERROR: MISSING DEPENDENCY ==========\033[0m"; \
		echo "\033[31mRequired command 'wget' not found in your PATH.\033[0m"; \
		echo ""; \
		echo "How to install wget:"; \
		echo "- Ubuntu/Debian: sudo apt-get install wget"; \
		echo "- macOS: brew install wget"; \
		echo "- Windows: Install using chocolatey with 'choco install wget' or download from https://eternallybored.org/misc/wget/"; \
		echo ""; \
		echo "After installing, run your command again."; \
		echo "\033[31m===============================================\033[0m"; \
		exit 1; \
	fi

.PHONY: ensure-tar
ensure-tar:
	@if ! command -v tar &> /dev/null; then \
		echo "\033[31m========== ERROR: MISSING DEPENDENCY ==========\033[0m"; \
		echo "\033[31mRequired command 'tar' not found in your PATH.\033[0m"; \
		echo ""; \
		echo "How to install tar:"; \
		echo "- Ubuntu/Debian: sudo apt-get install tar"; \
		echo "- macOS: Already included in the system"; \
		echo "- Windows: Install using chocolatey with 'choco install tar' or use Git Bash"; \
		echo ""; \
		echo "After installing, run your command again."; \
		echo "\033[31m===============================================\033[0m"; \
		exit 1; \
	fi

.PHONY: ensure-cargo-nextest
ensure-cargo-nextest:
	@if ! cargo nextest --version &> /dev/null; then \
		echo "\033[31m========== ERROR: MISSING DEPENDENCY ==========\033[0m"; \
		echo "\033[31mcargo-nextest not found.\033[0m"; \
		echo ""; \
		echo "To install cargo-nextest run:"; \
		echo "  cargo install cargo-nextest --locked"; \
		echo ""; \
		echo "After installing, run your command again."; \
		echo "\033[31m===============================================\033[0m"; \
		exit 1; \
	fi

.PHONY: ensure-ef-tests
ensure-ef-tests:
	@if [ ! -d $(EF_TESTS_DIR) ]; then \
		echo "\033[31m============= ERROR: MISSING DIRECTORY =============\033[0m"; \
		echo "\033[31mDirectory '$(EF_TESTS_DIR)' not found.\033[0m"; \
		echo ""; \
		echo "This might be because:"; \
		echo "1. You haven't downloaded the required files"; \
		echo "2. The download failed or was interrupted"; \
		echo "3. The files were moved or deleted"; \
		echo ""; \
		echo "How to fix:"; \
		echo "To download the Ethereum Foundation test vectors, run:"; \
		echo "  make ef-tests"; \
		echo ""; \
		echo "This will download and extract the test vectors from $(EF_TESTS_URL)"; \
		echo ""; \
		echo "\033[31m====================================================\033[0m"; \
		exit 1; \
	fi

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
	cargo install --path crates/optimism/bin --bin op-reth --force --locked \
		--features "$(FEATURES)" \
		--profile "$(PROFILE)" \
		$(CARGO_INSTALL_EXTRA_FLAGS)

.PHONY: build
build: ## Build the reth binary into `target` directory.
	cargo build --bin reth --features "$(FEATURES)" --profile "$(PROFILE)"

# Environment variables for reproducible builds
# Initialize RUSTFLAGS
RUST_BUILD_FLAGS =
# Enable static linking to ensure reproducibility across builds
RUST_BUILD_FLAGS += --C target-feature=+crt-static
# Set the linker to use static libgcc to ensure reproducibility across builds
RUST_BUILD_FLAGS += -Clink-arg=-static-libgcc
# Remove build ID from the binary to ensure reproducibility across builds
RUST_BUILD_FLAGS += -C link-arg=-Wl,--build-id=none
# Remove metadata hash from symbol names to ensure reproducible builds
RUST_BUILD_FLAGS += -C metadata=''
# Set timestamp from last git commit for reproducible builds
SOURCE_DATE ?= $(shell git log -1 --pretty=%ct)
# Disable incremental compilation to avoid non-deterministic artifacts
CARGO_INCREMENTAL_VAL = 0
# Set C locale for consistent string handling and sorting
LOCALE_VAL = C
# Set UTC timezone for consistent time handling across builds
TZ_VAL = UTC

.PHONY: build-reproducible
build-reproducible: ## Build the reth binary into `target` directory with reproducible builds. Only works for x86_64-unknown-linux-gnu currently
	SOURCE_DATE_EPOCH=$(SOURCE_DATE) \
	RUSTFLAGS="${RUST_BUILD_FLAGS} --remap-path-prefix $$(pwd)=." \
	CARGO_INCREMENTAL=${CARGO_INCREMENTAL_VAL} \
	LC_ALL=${LOCALE_VAL} \
	TZ=${TZ_VAL} \
	cargo build --bin reth --features "$(FEATURES)" --profile "release" --locked --target x86_64-unknown-linux-gnu

.PHONY: build-debug
build-debug: ## Build the reth binary into `target/debug` directory.
	cargo build --bin reth --features "$(FEATURES)"

.PHONY: build-op
build-op: ## Build the op-reth binary into `target` directory.
	cargo build --bin op-reth --features "$(FEATURES)" --profile "$(PROFILE)" --manifest-path crates/optimism/bin/Cargo.toml

# Builds the reth binary natively.
build-native-%:
	cargo build --bin reth --target $* --features "$(FEATURES)" --profile "$(PROFILE)"

op-build-native-%:
	cargo build --bin op-reth --target $* --features "$(FEATURES)" --profile "$(PROFILE)" --manifest-path crates/optimism/bin/Cargo.toml

# The following commands use `cross` to build a cross-compile.
#
# These commands require that:
#
# - `cross` is installed (`cargo install cross`).
# - Docker is running.
# - The current user is in the `docker` group.
#
# The resulting binaries will be created in the `target/` directory.

# For aarch64, set the page size for jemalloc.
# When cross compiling, we must compile jemalloc with a large page size,
# otherwise it will use the current system's page size which may not work
# on other systems. JEMALLOC_SYS_WITH_LG_PAGE=16 tells jemalloc to use 64-KiB
# pages. See: https://github.com/paradigmxyz/reth/issues/6742
build-aarch64-unknown-linux-gnu: export JEMALLOC_SYS_WITH_LG_PAGE=16
op-build-aarch64-unknown-linux-gnu: export JEMALLOC_SYS_WITH_LG_PAGE=16

# No jemalloc on Windows
build-x86_64-pc-windows-gnu: FEATURES := $(filter-out jemalloc jemalloc-prof,$(FEATURES))
op-build-x86_64-pc-windows-gnu: FEATURES := $(filter-out jemalloc jemalloc-prof,$(FEATURES))

# Note: The additional rustc compiler flags are for intrinsics needed by MDBX.
# See: https://github.com/cross-rs/cross/wiki/FAQ#undefined-reference-with-build-std
build-%:
	RUSTFLAGS="-C link-arg=-lgcc -Clink-arg=-static-libgcc" \
		cross build --bin reth --target $* --features "$(FEATURES)" --profile "$(PROFILE)"

op-build-%:
	RUSTFLAGS="-C link-arg=-lgcc -Clink-arg=-static-libgcc" \
		cross build --bin op-reth --target $* --features "$(FEATURES)" --profile "$(PROFILE)" --manifest-path crates/optimism/bin/Cargo.toml

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
op-build-x86_64-apple-darwin:
	$(MAKE) op-build-native-x86_64-apple-darwin
op-build-aarch64-apple-darwin:
	$(MAKE) op-build-native-aarch64-apple-darwin

# Create a `.tar.gz` containing a binary for a specific target.
define tarball_release_binary
	cp $(CARGO_TARGET_DIR)/$(1)/$(PROFILE)/$(2) $(BIN_DIR)/$(2)
	cd $(BIN_DIR) && \
		tar -czf reth-$(GIT_TAG)-$(1)$(3).tar.gz $(2) && \
		rm $(2)
endef

# The current git tag will be used as the version in the output file names. You
# will likely need to use `git tag` and create a semver tag (e.g., `v0.2.3`).
#
# Note: This excludes macOS tarballs because of SDK licensing issues.
.PHONY: build-release-tarballs
build-release-tarballs: ensure-tar ## Create a series of `.tar.gz` files in the BIN_DIR directory, each containing a `reth` binary for a different target.
	[ -d $(BIN_DIR) ] || mkdir -p $(BIN_DIR)
	$(MAKE) build-x86_64-unknown-linux-gnu
	$(call tarball_release_binary,"x86_64-unknown-linux-gnu","reth","")
	$(MAKE) build-aarch64-unknown-linux-gnu
	$(call tarball_release_binary,"aarch64-unknown-linux-gnu","reth","")
	$(MAKE) build-x86_64-pc-windows-gnu
	$(call tarball_release_binary,"x86_64-pc-windows-gnu","reth.exe","")

##@ Test

UNIT_TEST_ARGS := --locked --workspace --features 'jemalloc-prof' -E 'kind(lib)' -E 'kind(bin)' -E 'kind(proc-macro)'
COV_FILE := lcov.info

.PHONY: test-unit
test-unit: ensure-cargo-nextest ## Run unit tests.
	cargo nextest run $(UNIT_TEST_ARGS)


.PHONY: cov-unit
cov-unit: ensure-cargo-nextest ## Run unit tests with coverage.
	rm -f $(COV_FILE)
	cargo llvm-cov nextest --lcov --output-path $(COV_FILE) $(UNIT_TEST_ARGS)

.PHONY: cov-report-html
cov-report-html: cov-unit ## Generate a HTML coverage report and open it in the browser.
	cargo llvm-cov report --html
	open target/llvm-cov/html/index.html

# Downloads and unpacks Ethereum Foundation tests in the `$(EF_TESTS_DIR)` directory.
#
# Requires `wget` and `tar`
$(EF_TESTS_DIR): ensure-wget ensure-tar
	@echo "\033[33mDownloading Ethereum Foundation test vectors...\033[0m"
	@mkdir -p $(EF_TESTS_DIR)
	@if ! wget $(EF_TESTS_URL) -O ethereum-tests.tar.gz; then \
		echo "\033[31m========== ERROR: DOWNLOAD FAILED ==========\033[0m"; \
		echo "\033[31mFailed to download Ethereum Foundation test vectors.\033[0m"; \
		echo ""; \
		echo "This could be due to:"; \
		echo "1. No internet connection"; \
		echo "2. The URL might have changed ($(EF_TESTS_URL))"; \
		echo "3. GitHub might be temporarily unavailable"; \
		echo ""; \
		echo "Please check your internet connection and try again later."; \
		echo "\033[31m===========================================\033[0m"; \
		rm -rf $(EF_TESTS_DIR); \
		exit 1; \
	fi
	@if ! tar -xzf ethereum-tests.tar.gz --strip-components=1 -C $(EF_TESTS_DIR); then \
		echo "\033[31m========== ERROR: EXTRACTION FAILED ==========\033[0m"; \
		echo "\033[31mFailed to extract Ethereum Foundation test vectors.\033[0m"; \
		echo ""; \
		echo "This could be due to:"; \
		echo "1. Corrupted download"; \
		echo "2. Not enough disk space"; \
		echo "3. Permission issues"; \
		echo ""; \
		echo "Please try downloading again:"; \
		echo "  rm -rf $(EF_TESTS_DIR) ethereum-tests.tar.gz"; \
		echo "  make ef-tests"; \
		echo "\033[31m===============================================\033[0m"; \
		rm -f ethereum-tests.tar.gz; \
		rm -rf $(EF_TESTS_DIR); \
		exit 1; \
	fi
	@rm -f ethereum-tests.tar.gz
	@echo "\033[32mSuccessfully downloaded and extracted test vectors to $(EF_TESTS_DIR)\033[0m"

.PHONY: ef-tests
ef-tests: ensure-cargo-nextest ## Runs Ethereum Foundation tests.
	@if [ ! -d $(EF_TESTS_DIR) ]; then \
		echo "\033[31m========== ERROR: ETHEREUM TESTS MISSING ==========\033[0m"; \
		echo "Ethereum Foundation test vectors are required for these tests."; \
		echo ""; \
		echo "To download the test vectors (~1GB), run:"; \
		echo "  make ef-tests-download"; \
		echo ""; \
		echo "\033[31m===================================================\033[0m"; \
		exit 1; \
	fi
	@if [ ! -d $(EF_TESTS_DIR)/BlockchainTests ] || [ ! -d $(EF_TESTS_DIR)/GeneralStateTests ]; then \
		echo "\033[31m========== ERROR: INVALID TEST VECTORS ==========\033[0m"; \
		echo "The test vectors directory exists but has an invalid structure."; \
		echo ""; \
		echo "Try cleaning and redownloading:"; \
		echo "  make clean-ef-tests"; \
		echo "  make ef-tests-download"; \
		echo ""; \
		echo "\033[31m================================================\033[0m"; \
		exit 1; \
	fi
	@# Verify specific test directories exist to prevent multiple test failures
	@if [ ! -d $(EF_TESTS_DIR)/BlockchainTests/GeneralStateTests ]; then \
		echo "\033[31m========== ERROR: MISSING TEST SUBDIRECTORIES ==========\033[0m"; \
		echo "The GeneralStateTests subdirectory is missing from BlockchainTests."; \
		echo ""; \
		echo "Try downloading the test vectors again:"; \
		echo "  make clean-ef-tests"; \
		echo "  make ef-tests-download"; \
		echo ""; \
		echo "\033[31m======================================================\033[0m"; \
		exit 1; \
	fi
	@echo "\033[32mRunning Ethereum Foundation tests with properly installed test vectors...\033[0m"
	cargo nextest run -p ef-tests --features ef-tests

##@ Docker

# Note: This requires a buildx builder with emulation support. For example:
#
# `docker run --privileged --rm tonistiigi/binfmt --install amd64,arm64`
# `docker buildx create --use --driver docker-container --name cross-builder`
.PHONY: docker-build-push
docker-build-push: ## Build and push a cross-arch Docker image tagged with the latest git tag.
	$(call docker_build_push,$(GIT_TAG),$(GIT_TAG))

# Note: This requires a buildx builder with emulation support. For example:
#
# `docker run --privileged --rm tonistiigi/binfmt --install amd64,arm64`
# `docker buildx create --use --driver docker-container --name cross-builder`
.PHONY: docker-build-push-git-sha
docker-build-push-git-sha: ## Build and push a cross-arch Docker image tagged with the latest git sha.
	$(call docker_build_push,$(GIT_SHA),$(GIT_SHA))

# Note: This requires a buildx builder with emulation support. For example:
#
# `docker run --privileged --rm tonistiigi/binfmt --install amd64,arm64`
# `docker buildx create --use --driver docker-container --name cross-builder`
.PHONY: docker-build-push-latest
docker-build-push-latest: ## Build and push a cross-arch Docker image tagged with the latest git tag and `latest`.
	$(call docker_build_push,$(GIT_TAG),latest)

# Note: This requires a buildx builder with emulation support. For example:
#
# `docker run --privileged --rm tonistiigi/binfmt --install amd64,arm64`
# `docker buildx create --use --name cross-builder`
.PHONY: docker-build-push-nightly
docker-build-push-nightly: ## Build and push cross-arch Docker image tagged with the latest git tag with a `-nightly` suffix, and `latest-nightly`.
	$(call docker_build_push,nightly,nightly)

# Create a cross-arch Docker image with the given tags and push it
define docker_build_push
	$(MAKE) build-x86_64-unknown-linux-gnu
	mkdir -p $(BIN_DIR)/amd64
	cp $(CARGO_TARGET_DIR)/x86_64-unknown-linux-gnu/$(PROFILE)/reth $(BIN_DIR)/amd64/reth

	$(MAKE) build-aarch64-unknown-linux-gnu
	mkdir -p $(BIN_DIR)/arm64
	cp $(CARGO_TARGET_DIR)/aarch64-unknown-linux-gnu/$(PROFILE)/reth $(BIN_DIR)/arm64/reth

	docker buildx build --file ./Dockerfile.cross . \
		--platform linux/amd64,linux/arm64 \
		--tag $(DOCKER_IMAGE_NAME):$(1) \
		--tag $(DOCKER_IMAGE_NAME):$(2) \
		--provenance=false \
		--push
endef

##@ Optimism docker

# Note: This requires a buildx builder with emulation support. For example:
#
# `docker run --privileged --rm tonistiigi/binfmt --install amd64,arm64`
# `docker buildx create --use --driver docker-container --name cross-builder`
.PHONY: op-docker-build-push
op-docker-build-push: ## Build and push a cross-arch Docker image tagged with the latest git tag.
	$(call op_docker_build_push,$(GIT_TAG),$(GIT_TAG))

# Note: This requires a buildx builder with emulation support. For example:
#
# `docker run --privileged --rm tonistiigi/binfmt --install amd64,arm64`
# `docker buildx create --use --driver docker-container --name cross-builder`
.PHONY: op-docker-build-push-git-sha
op-docker-build-push-git-sha: ## Build and push a cross-arch Docker image tagged with the latest git sha.
	$(call op_docker_build_push,$(GIT_SHA),$(GIT_SHA))

# Note: This requires a buildx builder with emulation support. For example:
#
# `docker run --privileged --rm tonistiigi/binfmt --install amd64,arm64`
# `docker buildx create --use --driver docker-container --name cross-builder`
.PHONY: op-docker-build-push-latest
op-docker-build-push-latest: ## Build and push a cross-arch Docker image tagged with the latest git tag and `latest`.
	$(call op_docker_build_push,$(GIT_TAG),latest)

# Note: This requires a buildx builder with emulation support. For example:
#
# `docker run --privileged --rm tonistiigi/binfmt --install amd64,arm64`
# `docker buildx create --use --name cross-builder`
.PHONY: op-docker-build-push-nightly
op-docker-build-push-nightly: ## Build and push cross-arch Docker image tagged with the latest git tag with a `-nightly` suffix, and `latest-nightly`.
	$(call op_docker_build_push,nightly,nightly)

# Note: This requires a buildx builder with emulation support. For example:
#
# `docker run --privileged --rm tonistiigi/binfmt --install amd64,arm64`
# `docker buildx create --use --name cross-builder`
.PHONY: docker-build-push-nightly-profiling
docker-build-push-nightly-profiling: ## Build and push cross-arch Docker image with profiling profile tagged with nightly-profiling.
	$(call docker_build_push,nightly-profiling,nightly-profiling)

	# Note: This requires a buildx builder with emulation support. For example:
#
# `docker run --privileged --rm tonistiigi/binfmt --install amd64,arm64`
# `docker buildx create --use --name cross-builder`
.PHONY: op-docker-build-push-nightly-profiling
op-docker-build-push-nightly-profiling: ## Build and push cross-arch Docker image tagged with the latest git tag with a `-nightly` suffix, and `latest-nightly`.
	$(call op_docker_build_push,nightly-profiling,nightly-profiling)


# Create a cross-arch Docker image with the given tags and push it
define op_docker_build_push
	$(MAKE) op-build-x86_64-unknown-linux-gnu
	mkdir -p $(BIN_DIR)/amd64
	cp $(CARGO_TARGET_DIR)/x86_64-unknown-linux-gnu/$(PROFILE)/op-reth $(BIN_DIR)/amd64/op-reth

	$(MAKE) op-build-aarch64-unknown-linux-gnu
	mkdir -p $(BIN_DIR)/arm64
	cp $(CARGO_TARGET_DIR)/aarch64-unknown-linux-gnu/$(PROFILE)/op-reth $(BIN_DIR)/arm64/op-reth

	docker buildx build --file ./DockerfileOp.cross . \
		--platform linux/amd64,linux/arm64 \
		--tag $(DOCKER_IMAGE_NAME):$(1) \
		--tag $(DOCKER_IMAGE_NAME):$(2) \
		--provenance=false \
		--push
endef

##@ Other

.PHONY: clean
clean: ## Perform a `cargo` clean and remove the binary and test vectors directories.
	@echo "\033[33m=== Cleaning project ===\033[0m"
	cargo clean
	rm -rf $(BIN_DIR)
	@if [ -d $(EF_TESTS_DIR) ]; then \
		echo "\033[33mWARNING: Removing Ethereum Foundation test vectors at $(EF_TESTS_DIR)\033[0m"; \
		echo "These will need to be redownloaded before running tests (>1GB download)."; \
		rm -rf $(EF_TESTS_DIR); \
	fi
	@echo "\033[32m=== Project cleaned ===\033[0m"
	@echo "To re-download test vectors, run: make ef-tests"

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
update-book-cli: build-debug ## Update book cli documentation.
	@echo "Updating book cli doc..."
	@./book/cli/update.sh $(CARGO_TARGET_DIR)/debug/reth

.PHONY: profiling
profiling: ## Builds `reth` with optimisations, but also symbols.
	RUSTFLAGS="-C target-cpu=native" cargo build --profile profiling --features jemalloc,asm-keccak

.PHONY: profiling-op
profiling-op: ## Builds `op-reth` with optimisations, but also symbols.
	RUSTFLAGS="-C target-cpu=native" cargo build --profile profiling --features jemalloc,asm-keccak --bin op-reth --manifest-path crates/optimism/bin/Cargo.toml

.PHONY: maxperf
maxperf: ## Builds `reth` with the most aggressive optimisations.
	RUSTFLAGS="-C target-cpu=native" cargo build --profile maxperf --features jemalloc,asm-keccak

.PHONY: maxperf-op
maxperf-op: ## Builds `op-reth` with the most aggressive optimisations.
	RUSTFLAGS="-C target-cpu=native" cargo build --profile maxperf --features jemalloc,asm-keccak --bin op-reth --manifest-path crates/optimism/bin/Cargo.toml

.PHONY: maxperf-no-asm
maxperf-no-asm: ## Builds `reth` with the most aggressive optimisations, minus the "asm-keccak" feature.
	RUSTFLAGS="-C target-cpu=native" cargo build --profile maxperf --features jemalloc


fmt:
	cargo +nightly fmt

clippy:
	cargo +nightly clippy \
	--workspace \
	--lib \
	--examples \
	--tests \
	--benches \
	--all-features \
	-- -D warnings

lint-codespell: ensure-codespell
	codespell --skip "*.json" --skip "./testing/ef-tests/ethereum-tests"

ensure-codespell:
	@if ! command -v codespell &> /dev/null; then \
		echo "codespell not found. Please install it by running the command `pip install codespell` or refer to the following link for more information: https://github.com/codespell-project/codespell" \
		exit 1; \
    fi

# Lint and format all TOML files in the project using dprint.
# This target ensures that TOML files follow consistent formatting rules,
# such as using spaces instead of tabs, and enforces other style guidelines
# defined in the dprint configuration file (e.g., dprint.json).
#
# Usage:
#   make lint-toml
#
# Dependencies:
#   - ensure-dprint: Ensures that dprint is installed and available in the system.
lint-toml: ensure-dprint
	dprint fmt

ensure-dprint:
	@if ! command -v dprint &> /dev/null; then \
		echo "dprint not found. Please install it by running the command `cargo install --locked dprint` or refer to the following link for more information: https://github.com/dprint/dprint" \
		exit 1; \
    fi

lint:
	make fmt && \
	make clippy && \
	make lint-codespell && \
	make lint-toml

clippy-fix:
	cargo +nightly clippy \
	--workspace \
	--lib \
	--examples \
	--tests \
	--benches \
	--all-features \
	--fix \
	--allow-staged \
	--allow-dirty \
	-- -D warnings

fix-lint:
	make clippy-fix && \
	make fmt

.PHONY: rustdocs
rustdocs: ## Runs `cargo docs` to generate the Rust documents in the `target/doc` directory
	RUSTDOCFLAGS="\
	--cfg docsrs \
	--show-type-layout \
	--generate-link-to-definition \
	--enable-index-page -Zunstable-options -D warnings" \
	cargo +nightly docs \
	--document-private-items

cargo-test:
	@echo "\033[1m=== Running regular cargo tests (NOT including EF tests) ===\033[0m"
	cargo test \
	--workspace \
	--exclude ef-tests \
	--bin "op-reth" \
	--lib --examples \
	--tests \
	--benches \
	--all-features
	@echo "\033[33mNote: Ethereum Foundation tests were excluded from this run.\033[0m"
	@echo "To run all tests including EF tests, use: make test"

test-doc:
	cargo test --doc --workspace --all-features

.PHONY: test
test: ## Run all tests including EF tests
	@echo "\033[1m=== Running regular cargo tests ===\033[0m"
	make cargo-test
	@echo "\033[1m=== Running doc tests ===\033[0m"
	make test-doc
	@echo "\033[1m=== Running Ethereum Foundation tests ===\033[0m"
	@make check-ef-tests || (echo "\033[31mSkipping EF tests due to missing test vectors\033[0m" && exit 1)
	make ef-tests

# Check if EF tests are available and correctly downloaded
.PHONY: check-ef-tests
check-ef-tests: ensure-cargo-nextest
	@if [ ! -d $(EF_TESTS_DIR) ]; then \
		echo "\033[31m========== ERROR: ETHEREUM TESTS MISSING ==========\033[0m"; \
		echo "Ethereum Foundation test vectors are missing."; \
		echo ""; \
		echo "These test vectors are REQUIRED to run the PR validation."; \
		echo ""; \
		echo "To download them, run:"; \
		echo "  make ef-tests-download"; \
		echo ""; \
		echo "This will download ~1GB of test vectors."; \
		echo "\033[31m===================================================\033[0m"; \
		exit 1; \
	fi
	@if [ ! -d $(EF_TESTS_DIR)/BlockchainTests ] || [ ! -d $(EF_TESTS_DIR)/GeneralStateTests ]; then \
		echo "\033[31m========== ERROR: INCOMPLETE TEST VECTORS ==========\033[0m"; \
		echo "The test vectors directory exists but is missing key subdirectories."; \
		echo ""; \
		echo "To fix this issue:"; \
		echo "  make clean-ef-tests"; \
		echo "  make ef-tests-download"; \
		echo ""; \
		echo "This will download ~1GB of test vectors."; \
		echo "\033[31m====================================================\033[0m"; \
		exit 1; \
	fi
	@echo "\033[32mEthereum Foundation test vectors are correctly installed.\033[0m"

.PHONY: clean-ef-tests
clean-ef-tests: ## Remove Ethereum Foundation test vectors to free up disk space
	@echo "\033[33m=== Cleaning Ethereum Foundation test vectors ===\033[0m"
	@if [ ! -d $(EF_TESTS_DIR) ]; then \
		echo "No test vectors found at $(EF_TESTS_DIR). Nothing to clean."; \
		exit 0; \
	fi
	@echo "Removing test vectors directory ($(EF_TESTS_DIR))..."
	@rm -rf $(EF_TESTS_DIR)
	@rm -f ethereum-tests.tar.gz
	@echo "\033[32m=== Test vectors removed successfully ===\033[0m"
	@echo "Test vectors will need to be redownloaded before running tests."
	@echo "To download them again, run: make ef-tests-download"

.PHONY: ef-tests-download
ef-tests-download: ensure-wget ensure-tar ## Download Ethereum Foundation test vectors (required for running tests)
	@echo "\033[1m=== Downloading Ethereum Foundation test vectors ===\033[0m"
	@echo "This will download around 1GB of test vectors, which is required for running tests."
	@echo "Download will start in 3 seconds..."
	@sleep 3
	@if [ -d $(EF_TESTS_DIR) ]; then \
		echo "\033[33mWARNING: Test vectors directory already exists at $(EF_TESTS_DIR)\033[0m"; \
		echo "To force a clean download, run:"; \
		echo "  make clean-ef-tests"; \
		echo "  make ef-tests-download"; \
		exit 1; \
	fi
	@echo "\033[33mStarting download of Ethereum Foundation test vectors...\033[0m"
	@$(MAKE) $(EF_TESTS_DIR)
	@if [ -d $(EF_TESTS_DIR)/BlockchainTests ] && [ -d $(EF_TESTS_DIR)/GeneralStateTests ]; then \
		echo "\033[32m=== Test vectors downloaded successfully! ===\033[0m"; \
		echo "You can now run: make test"; \
	else \
		echo "\033[31m=== Downloaded test vectors are incomplete ===\033[0m"; \
		echo "The download appears to have failed or the test repository structure has changed."; \
		echo ""; \
		echo "Try cleaning and redownloading:"; \
		echo "  make clean-ef-tests"; \
		echo "  make ef-tests-download"; \
		exit 1; \
	fi

check-features:
	cargo hack check \
		--package reth-codecs \
		--package reth-primitives-traits \
		--package reth-primitives \
		--feature-powerset

.PHONY: check-dependencies
check-dependencies: ## Check all required dependencies
	@echo "\033[1m=== Checking required dependencies ===\033[0m"
	@ERROR_COUNT=0; \
	if ! command -v wget &> /dev/null; then \
		echo "\033[31mERROR: wget not found\033[0m"; \
		echo "To install wget:"; \
		echo "- Ubuntu/Debian: sudo apt-get install wget"; \
		echo "- macOS: brew install wget"; \
		echo "- Windows: choco install wget or download from https://eternallybored.org/misc/wget/"; \
		echo ""; \
		ERROR_COUNT=$$((ERROR_COUNT+1)); \
	else \
		echo "\033[32m✓ wget is installed\033[0m"; \
	fi; \
	if ! command -v tar &> /dev/null; then \
		echo "\033[31mERROR: tar not found\033[0m"; \
		echo "To install tar:"; \
		echo "- Ubuntu/Debian: sudo apt-get install tar"; \
		echo "- macOS: Already included in the system"; \
		echo "- Windows: choco install tar or use Git Bash"; \
		echo ""; \
		ERROR_COUNT=$$((ERROR_COUNT+1)); \
	else \
		echo "\033[32m✓ tar is installed\033[0m"; \
	fi; \
	if ! cargo nextest --version &> /dev/null; then \
		echo "\033[31mERROR: cargo-nextest not found\033[0m"; \
		echo "To install cargo-nextest:"; \
		echo "  cargo install cargo-nextest --locked"; \
		echo ""; \
		ERROR_COUNT=$$((ERROR_COUNT+1)); \
	else \
		echo "\033[32m✓ cargo-nextest is installed\033[0m"; \
	fi; \
	if [ ! -d $(EF_TESTS_DIR) ]; then \
		echo "\033[33mWARNING: Ethereum Foundation test vectors not found\033[0m"; \
		echo "To download them:"; \
		echo "  make ef-tests"; \
		echo ""; \
	else \
		echo "\033[32m✓ Ethereum Foundation test vectors are available\033[0m"; \
		if [ ! -d $(EF_TESTS_DIR)/BlockchainTests ] || [ ! -d $(EF_TESTS_DIR)/GeneralStateTests ]; then \
			echo "\033[31mERROR: Test vectors appear to be incomplete or corrupted\033[0m"; \
			echo "To fix:"; \
			echo "  rm -rf $(EF_TESTS_DIR)"; \
			echo "  make ef-tests"; \
			echo ""; \
			ERROR_COUNT=$$((ERROR_COUNT+1)); \
		fi; \
	fi; \
	if [ $$ERROR_COUNT -gt 0 ]; then \
		echo "\033[31m=== Found $$ERROR_COUNT error(s) ===\033[0m"; \
		echo "Run 'make setup' to fix installation issues."; \
		exit 1; \
	else \
		echo "\033[32m=== All required dependencies are properly installed ===\033[0m"; \
	fi

.PHONY: setup
setup: ## Install or verify all required dependencies
	@echo "\033[1m=== Setting up required dependencies ===\033[0m"
	
	@# Check for Cargo
	@if ! command -v cargo &> /dev/null; then \
		echo "\033[31m========== ERROR: RUST NOT INSTALLED ==========\033[0m"; \
		echo "\033[31mRust and Cargo not found\033[0m"; \
		echo ""; \
		echo "Rust is required to build and test this project."; \
		echo ""; \
		echo "To install Rust:"; \
		echo "  Visit https://rustup.rs/ and follow the instructions"; \
		echo "  Or run 'curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh'"; \
		echo ""; \
		echo "After installing, restart your terminal and run 'make setup' again."; \
		echo "\033[31m=================================================\033[0m"; \
		exit 1; \
	else \
		echo "\033[32m✓ Rust and Cargo are installed\033[0m"; \
	fi
	
	@# Check for wget
	@if ! command -v wget &> /dev/null; then \
		echo "\033[33m=== wget not found, installation required ===\033[0m"; \
		echo "Installing wget is system-dependent and must be done manually:"; \
		echo "- Ubuntu/Debian: sudo apt-get install wget"; \
		echo "- macOS: brew install wget"; \
		echo "- Windows: choco install wget or download from https://eternallybored.org/misc/wget/"; \
		echo ""; \
		echo "Please install wget and run 'make setup' again."; \
		exit 1; \
	else \
		echo "\033[32m✓ wget is installed\033[0m"; \
	fi
	
	@# Check for tar
	@if ! command -v tar &> /dev/null; then \
		echo "\033[33m=== tar not found, installation required ===\033[0m"; \
		echo "Installing tar is system-dependent and must be done manually:"; \
		echo "- Ubuntu/Debian: sudo apt-get install tar"; \
		echo "- macOS: Already included in the system"; \
		echo "- Windows: choco install tar or use Git Bash"; \
		echo ""; \
		echo "Please install tar and run 'make setup' again."; \
		exit 1; \
	else \
		echo "\033[32m✓ tar is installed\033[0m"; \
	fi
	
	@# Check for cargo-nextest
	@if ! cargo nextest --version &> /dev/null; then \
		echo "\033[33m=== Installing cargo-nextest ===\033[0m"; \
		if ! cargo install cargo-nextest --locked; then \
			echo "\033[31mFailed to install cargo-nextest\033[0m"; \
			echo ""; \
			echo "This could be due to:"; \
			echo "1. Network issues"; \
			echo "2. Insufficient permissions"; \
			echo "3. Disk space issues"; \
			echo ""; \
			echo "Try running the following command manually:"; \
			echo "  cargo install cargo-nextest --locked"; \
			exit 1; \
		fi; \
		echo "\033[32m✓ cargo-nextest installed successfully\033[0m"; \
	else \
		echo "\033[32m✓ cargo-nextest is already installed\033[0m"; \
	fi
	
	@# Check for EF tests
	@if [ ! -d $(EF_TESTS_DIR) ]; then \
		echo "\033[33m=== Downloading Ethereum Foundation test vectors ===\033[0m"; \
		if ! $(MAKE) $(EF_TESTS_DIR); then \
			echo "\033[31mFailed to download EF test vectors\033[0m"; \
			echo ""; \
			echo "Try downloading them manually later using:"; \
			echo "  make ef-tests"; \
			exit 1; \
		fi; \
	else \
		if [ ! -d $(EF_TESTS_DIR)/BlockchainTests ] || [ ! -d $(EF_TESTS_DIR)/GeneralStateTests ]; then \
			echo "\033[33m=== Test vectors appear incomplete, redownloading ===\033[0m"; \
			rm -rf $(EF_TESTS_DIR); \
			if ! $(MAKE) $(EF_TESTS_DIR); then \
				echo "\033[31mFailed to download EF test vectors\033[0m"; \
				echo ""; \
				echo "Try downloading them manually later using:"; \
				echo "  make ef-tests"; \
				exit 1; \
			fi; \
		else \
			echo "\033[32m✓ Ethereum Foundation test vectors are already downloaded\033[0m"; \
		fi; \
	fi
	
	@echo "\033[32m=== All dependencies have been set up successfully! ===\033[0m"
	@echo "You can now run tests with: make test"

.PHONY: pr
pr: ## Full check for PR validation
	@make check-ef-tests || exit 1
	make lint
	make update-book-cli
	cargo docs --document-private-items
	make test

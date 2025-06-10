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
EF_TESTS_TAG := v17.0
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
RUST_BUILD_FLAGS += -C link-arg=-static-libgcc
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
COV_FILE := lcov.info

.PHONY: test-unit
test-unit: ## Run unit tests.
	cargo install cargo-nextest --locked
	cargo nextest run $(UNIT_TEST_ARGS)


.PHONY: cov-unit
cov-unit: ## Run unit tests with coverage.
	rm -f $(COV_FILE)
	cargo llvm-cov nextest --lcov --output-path $(COV_FILE) $(UNIT_TEST_ARGS)

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

##@ Profile-Guided Optimization (PGO) + BOLT

# This section provides targets for building reth with Profile-Guided
# Optimization (PGO) and BOLT (Binary Optimization and Layout Tool).
#
# This workflow relies on `cargo-pgo` for PGO and `llvm-bolt` for BOLT. The
# workflow was brought mainly from this post by the `cargo-pgo` author:
# https://kobzol.github.io/rust/cargo/2023/07/28/rust-cargo-pgo.html
#
# More information on installing the tools can be found in the `cargo-pgo` repo:
# https://github.com/Kobzol/cargo-pgo

# Directory for PGO data
PGO_DATA_DIR ?= $(CARGO_TARGET_DIR)/pgo-data
BOLT_DATA_DIR ?= $(CARGO_TARGET_DIR)/bolt-data

# Check for required tools
.PHONY: pgo-check-deps
pgo-check-deps: ## Check if required tools are installed.
	@command -v cargo-pgo >/dev/null 2>&1 || { echo "Error: cargo-pgo not found. Install with: cargo install cargo-pgo"; exit 1; }
	@command -v llvm-bolt >/dev/null 2>&1 || { echo "Error: llvm-bolt not found. Please install LLVM tools with BOLT support."; exit 1; }
	@command -v perf >/dev/null 2>&1 || { echo "Error: perf not found. Please install perf for profiling."; exit 1; }
	@command -v merge-fdata >/dev/null 2>&1 || { echo "Error: merge-fdata not found. Please install llvm-tools or llvm-profdata."; exit 1; }

.PHONY: pgo-build-instrumented
pgo-build-instrumented: pgo-check-deps ## Build reth with PGO instrumentation.
	@echo "Building reth with PGO instrumentation using cargo-pgo..."
	@mkdir -p $(PGO_DATA_DIR)
	CARGO_PGO_DATA_DIR=$(PGO_DATA_DIR) \
		cargo pgo build --profile maxperf --features "$(FEATURES)" --bin reth

.PHONY: pgo-run
pgo-run: ## Run reth to collect PGO data. You must set PGO_RUN_CMD environment variable.
	@test -n "$(PGO_RUN_CMD)" || { echo "Error: PGO_RUN_CMD must be set. Example: PGO_RUN_CMD='timeout 300 ./target/maxperf/reth node --datadir $(CARGO_TARGET_DIR)/pgo-datadir --chain mainnet'"; exit 1; }
	@echo "Running reth to collect PGO data..."
	CARGO_PGO_DATA_DIR=$(PGO_DATA_DIR) \
		cargo pgo run --profile maxperf --features "$(FEATURES)" --bin reth -- $(PGO_RUN_CMD)

.PHONY: pgo-optimize
pgo-optimize: ## Build PGO-optimized reth binary.
	@echo "Building PGO-optimized reth..."
	CARGO_PGO_DATA_DIR=$(PGO_DATA_DIR) \
		cargo pgo optimize --profile maxperf --features "$(FEATURES)" --bin reth

.PHONY: bolt-prepare
bolt-prepare: pgo-optimize ## Prepare for BOLT optimization by collecting performance data.
	@test -n "$(BOLT_RUN_CMD)" || { echo "Error: BOLT_RUN_CMD must be set. Example: BOLT_RUN_CMD='timeout 300 ./target/maxperf/reth node --datadir $(CARGO_TARGET_DIR)/bolt-datadir --chain mainnet'"; exit 1; }
	@echo "Collecting BOLT profile data..."
	@mkdir -p $(BOLT_DATA_DIR)
	perf record -e cycles:u -j any,u -o $(BOLT_DATA_DIR)/perf.data -- $(BOLT_RUN_CMD) || echo "BOLT profiling completed"

.PHONY: bolt-optimize
bolt-optimize: bolt-prepare ## Apply BOLT optimization to the PGO-optimized binary.
	@echo "Converting perf data for BOLT..."
	perf2bolt $(CARGO_TARGET_DIR)/maxperf/reth -p $(BOLT_DATA_DIR)/perf.data -o $(BOLT_DATA_DIR)/reth.fdata
	@echo "Applying BOLT optimization..."
	llvm-bolt $(CARGO_TARGET_DIR)/maxperf/reth \
		-data $(BOLT_DATA_DIR)/reth.fdata \
		-reorder-blocks=ext-tsp \
		-reorder-functions=hfsort+ \
		-split-functions \
		-split-all-cold \
		-split-eh \
		-dyno-stats \
		-icf=1 \
		-use-gnu-stack \
		-o $(CARGO_TARGET_DIR)/maxperf/reth-bolt
	@echo "BOLT-optimized binary created: $(CARGO_TARGET_DIR)/maxperf/reth-bolt"

# Example commands for reference:
# PGO_RUN_CMD="timeout 300 ./target/maxperf/reth node --datadir target/pgo-datadir --chain mainnet"
# BOLT_RUN_CMD="timeout 300 ./target/maxperf/reth node --datadir target/bolt-datadir --chain mainnet"
# For better results, specify a recent mainnet block hash to sync heavier blocks:
# PGO_RUN_CMD="timeout 300 ./target/maxperf/reth node --datadir target/pgo-datadir --debug.tip 0xYOUR_BLOCK_HASH"

.PHONY: pgo-clean
pgo-clean: ## Clean PGO and BOLT data and temporary directories.
	@echo "Cleaning PGO and BOLT data..."
	rm -rf $(PGO_DATA_DIR)
	rm -rf $(BOLT_DATA_DIR)
	rm -rf $(CARGO_TARGET_DIR)/pgo-datadir
	rm -rf $(CARGO_TARGET_DIR)/bolt-datadir
	rm -f $(CARGO_TARGET_DIR)/maxperf/reth-bolt

.PHONY: pgo-bolt
pgo-bolt: pgo-clean pgo-build-instrumented pgo-run pgo-optimize bolt-optimize ## Run full PGO+BOLT workflow: instrument, collect PGO data, optimize with PGO, then optimize with BOLT.
	@echo "PGO+BOLT build complete!"
	@echo "PGO-optimized binary: $(CARGO_TARGET_DIR)/maxperf/reth"
	@echo "BOLT-optimized binary: $(CARGO_TARGET_DIR)/maxperf/reth-bolt"

.PHONY: pgo-only
pgo-only: pgo-clean pgo-build-instrumented pgo-run pgo-optimize ## Run PGO-only workflow without BOLT.
	@echo "PGO build complete! Binary location: $(CARGO_TARGET_DIR)/maxperf/reth"

# Native PGO targets for macOS (without cargo-pgo)
.PHONY: pgo-native-build
pgo-native-build: ## Build reth with PGO instrumentation using native cargo (macOS compatible).
	@echo "Building reth with PGO instrumentation using native cargo..."
	@mkdir -p $(PGO_DATA_DIR)
	RUSTFLAGS="-Cprofile-generate=$(PGO_DATA_DIR) -Cllvm-args=-vp-counters-per-site=64" \
		cargo build --profile maxperf --features "$(FEATURES)" --bin reth
	@echo "PGO instrumented build complete!"
	@echo "Binary: $(CARGO_TARGET_DIR)/maxperf/reth"
	@echo "PGO data will be written to: $(PGO_DATA_DIR)"

.PHONY: pgo-native-optimize
pgo-native-optimize: ## Build PGO-optimized reth using native cargo (macOS compatible).
	@echo "Building PGO-optimized reth using native cargo..."
	@test -d $(PGO_DATA_DIR) || { echo "Error: No PGO data found in $(PGO_DATA_DIR). Run pgo-native-build and collect profile data first."; exit 1; }
	@if command -v llvm-profdata >/dev/null 2>&1; then \
		echo "Merging PGO data..."; \
		llvm-profdata merge -o $(PGO_DATA_DIR)/merged.profdata $(PGO_DATA_DIR)/*.profraw; \
		RUSTFLAGS="-Cprofile-use=$$(pwd)/$(PGO_DATA_DIR)/merged.profdata" \
			cargo build --profile maxperf --features "$(FEATURES)" --bin reth; \
	else \
		echo "Warning: llvm-profdata not found, using unmerged profile data..."; \
		RUSTFLAGS="-Cprofile-use=$$(pwd)/$(PGO_DATA_DIR)" \
			cargo build --profile maxperf --features "$(FEATURES)" --bin reth; \
	fi
	@echo "PGO optimization complete!"
	@echo "Optimized binary: $(CARGO_TARGET_DIR)/maxperf/reth"

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

clippy-op-dev:
	cargo +nightly clippy \
	--bin op-reth \
	--workspace \
	--lib \
	--examples \
	--tests \
	--benches \
	--locked \
	--all-features

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
	cargo test \
	--workspace \
	--bin "op-reth" \
	--lib --examples \
	--tests \
	--benches \
	--all-features

test-doc:
	cargo test --doc --workspace --all-features

test:
	make cargo-test && \
	make test-doc

pr:
	make lint && \
	make update-book-cli && \
	cargo docs --document-private-items && \
	make test

check-features:
	cargo hack check \
		--package reth-codecs \
		--package reth-primitives-traits \
		--package reth-primitives \
		--feature-powerset

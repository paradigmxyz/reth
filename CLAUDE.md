# Claude Code Agent Guide for Reth

This guide provides comprehensive instructions for Claude Code agents working on Reth, the modular, contributor-friendly and blazing-fast implementation of the Ethereum protocol in Rust.

## Project Overview

**Reth** (short for Rust Ethereum) is a new Ethereum full node implementation focused on being user-friendly, highly modular, fast, and efficient. It's an Execution Layer (EL) compatible with all Ethereum Consensus Layer (CL) implementations supporting the Engine API.

### Key Characteristics
- **Production Ready**: Suitable for mission-critical environments (staking, RPC, MEV, indexing)
- **Modular Design**: Every component built as a reusable library
- **High Performance**: Uses Rust and Erigon's staged-sync architecture
- **Apache/MIT Licensed**: Free open source software
- **Client Diversity**: Contributes to Ethereum's antifragility

## Development Environment Setup

### Prerequisites

**Minimum Supported Rust Version (MSRV)**: 1.86.0

```bash
# Install Rust with rustup
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install nightly for formatting and advanced clippy features
rustup install nightly

# Install required tools
cargo install cargo-nextest --locked
cargo install cross  # For cross-compilation (optional)
```

### Dependencies

- **Required**: Git, Rust 1.86+
- **Recommended**: [`cargo-nextest`](https://nexte.st/) for faster testing
- **Optional**: [Geth](https://geth.ethereum.org/docs/getting-started/installing-geth) for complete test suite
- **Cross-compilation**: Docker + `cross` for multi-platform builds

### Clone and Setup

```bash
git clone https://github.com/paradigmxyz/reth
cd reth
```

## Code Style and Quality

### Formatting

Reth uses **nightly rustfmt** with specific configuration:

```bash
# Format code (REQUIRED before commits)
make fmt

# Check formatting without changes  
cargo +nightly fmt --all --check
```

**Key formatting rules** (from `rustfmt.toml`):
- `reorder_imports = true` - Imports grouped by crate
- `imports_granularity = "Crate"` - One use statement per crate
- `trailing_comma = "Vertical"` - Trailing commas in vertical layouts
- `comment_width = 100` - Wrap comments at 100 characters
- `wrap_comments = true` - Wrap long comments

### Linting with Clippy

Reth uses **nightly clippy** with strict warnings:

```bash
# Run clippy (REQUIRED before commits)
make clippy

# Auto-fix issues where possible
make clippy-fix

# For Optimism development
make clippy-op-dev
```

**Clippy configuration** (from `clippy.toml`):
- `msrv = "1.86"` - Respect MSRV
- `too-large-for-stack = 128` - Warn on large stack types
- `allow-dbg-in-tests = true` - Allow `dbg!` in tests
- Custom doc identifiers: `P2P`, `ExEx`, `IPv4`, `IPv6`, etc.

### Complete Lint Check

```bash
# Run all linting steps
make lint

# Auto-fix all linting issues
make fix-lint
```

## Build Commands

### Basic Builds

```bash
# Debug build
cargo build --bin reth --features "jemalloc asm-keccak min-debug-logs"

# Release build  
cargo build --bin reth --features "jemalloc asm-keccak min-debug-logs" --release

# Using Makefile (recommended)
make build        # Release build
make build-debug  # Debug build
```

### Optimized Builds

```bash
# Profiling build (optimized + symbols)
make profiling
# Equivalent to:
RUSTFLAGS="-C target-cpu=native" cargo build --profile profiling --features jemalloc,asm-keccak

# Maximum performance build
make maxperf
# Equivalent to:  
RUSTFLAGS="-C target-cpu=native" cargo build --profile maxperf --features jemalloc,asm-keccak

# Without asm-keccak (for compatibility)
make maxperf-no-asm
```

### Cross-Platform Builds

```bash
# Install cross
cargo install cross

# Build for specific targets
make build-x86_64-unknown-linux-gnu
make build-aarch64-unknown-linux-gnu
make build-x86_64-pc-windows-gnu

# Note: Windows builds exclude jemalloc automatically
# Note: aarch64 builds set JEMALLOC_SYS_WITH_LG_PAGE=16 for 64-KiB pages
```

### Features

**Default features** (Linux/macOS):
- `jemalloc` - Memory allocator
- `asm-keccak` - Assembly-optimized Keccak
- `min-debug-logs` - Minimal debug logging

**Windows** (jemalloc not supported):
- `asm-keccak`  
- `min-debug-logs`

**Additional features**:
- `jemalloc-prof` - Profiling support
- Various feature combinations for specific use cases

## Testing

### Unit Tests

```bash
# Run unit tests (recommended)
cargo nextest run --workspace

# Run with coverage
make cov-unit

# Generate HTML coverage report
make cov-report-html
```

### Ethereum Foundation Tests

```bash
# Download and run EF tests
make ef-tests
```

**Note**: EF tests are downloaded automatically to `./testing/ef-tests/ethereum-tests/` from tag `v17.0`.

### Integration Tests

```bash
# Full test suite
make test

# Manual equivalent
make cargo-test && make test-doc
```

### Test Configuration

- Uses `cargo-nextest` for improved performance and retries
- Random seed can be set via `SEED` environment variable for deterministic tests
- Some tests require Geth to be installed

## Benchmarking

### Running Benchmarks

```bash
# Run all benchmarks
cargo bench --workspace

# Run specific benchmark
cargo bench --workspace 'benchmark_name'

# Run with specific features
cargo bench --workspace --features 'jemalloc-prof'
```

### Benchmark Organization

Benchmarks are located in:
- `crates/*/benches/` - Crate-specific benchmarks
- Root `benches/` - Cross-crate benchmarks

**Important**: Always use existing benchmark infrastructure rather than creating standalone test files.

## Crate Architecture

Reth follows a **modular architecture** with clear separation of concerns:

### Core Components

#### Storage (`crates/storage/`)
- **`codecs/`** - Storage encoding/decoding
- **`db/`** - Database abstractions (MDBX backend)
- **`libmdbx-rs/`** - Rust bindings for libmdbx
- **`provider/`** - High-level data access traits
- **`nippy-jar/`** - Compression utilities

#### Networking (`crates/net/`)
- **`network/`** - Main networking component (P2P, peers, sessions)
- **`discv4/`** - Node discovery protocol
- **`discv5/`** - Enhanced discovery with ENR
- **`dns/`** - DNS-based discovery (EIP-1459)
- **`eth-wire/`** - Ethereum wire protocol + RLPx
- **`downloaders/`** - Block/header download logic

#### Consensus (`crates/consensus/`)
- **`common/`** - Shared consensus utilities
- **`consensus/`** - Consensus mechanism traits

#### Execution (`crates/evm/`)
- **`evm/`** - EVM configuration and execution
- **`execution-types/`** - Common execution types
- **`execution-errors/`** - Execution error types

#### Node (`crates/node/`)
- **`builder/`** - Node builder pattern and setup
- **`api/`** - Node API traits
- **`core/`** - Core node functionality
- **`events/`** - Node event system

#### RPC (`crates/rpc/`)
- **`rpc-api/`** - RPC API definitions
- **`rpc-eth-api/`** - Ethereum JSON-RPC methods
- **`rpc-engine-api/`** - Engine API implementation
- **`rpc-builder/`** - RPC server construction

### Supporting Crates

#### Primitives (`crates/primitives*`)
- **`primitives/`** - Core Ethereum types (blocks, transactions)
- **`primitives-traits/`** - Essential traits and interfaces

#### Utilities
- **`tasks/`** - Async task management
- **`metrics/`** - Prometheus metrics
- **`tracing/`** - Logging and instrumentation
- **`fs-util/`** - Filesystem utilities

### Chain-Specific

#### Ethereum (`crates/ethereum/`)
- **`consensus/`** - Ethereum consensus validation
- **`evm/`** - Ethereum-specific EVM config
- **`hardforks/`** - Ethereum hardfork logic

#### Optimism (`crates/optimism/`)
- **`node/`** - OP Stack node implementation
- **`evm/`** - Optimism EVM modifications
- **`consensus/`** - OP Stack consensus rules

## Common Development Tasks

### Adding a New Feature

1. **Identify the appropriate crate(s)** based on the architecture
2. **Follow existing patterns** in similar crates
3. **Add comprehensive tests** including unit and integration tests
4. **Update documentation** if adding public APIs
5. **Run full test suite** before submitting

### Performance Optimization

1. **Use existing benchmarks** - Don't create standalone test files
2. **Measure before optimizing** - Get baseline metrics
3. **Profile with the right tools**:
   ```bash
   make profiling  # Build with symbols
   # Use tools like perf, valgrind, flamegraph
   ```
4. **Document performance changes** with concrete measurements

### Database Changes

- Database models in `crates/storage/db-models/`
- Migrations handled through versioning
- Always test with existing data
- Consider backward compatibility

### Adding RPC Methods

1. **Define the API** in `crates/rpc/rpc-api/`
2. **Implement in appropriate namespace** (`eth`, `debug`, `trace`, etc.)
3. **Add comprehensive tests** including edge cases
4. **Update documentation** in `book/jsonrpc/`

## Quality Assurance

### Pre-commit Checklist

```bash
# 1. Format code
make fmt

# 2. Run clippy
make clippy

# 3. Run tests
make test-unit

# 4. Run additional lints
make lint-codespell  # Spell checking
make lint-toml       # TOML formatting

# 5. Update CLI docs if needed
make update-book-cli

# 6. Run docs generation
make rustdocs

# Or run all quality checks at once:
make lint && make test
```

### CI/CD Pipeline

The project uses GitHub Actions for:
- **Unit tests** across multiple Rust versions
- **Integration tests** with Ethereum Foundation test vectors
- **Cross-compilation** for multiple targets
- **Linting** (clippy, formatting, spelling)
- **Security audits** with `cargo deny`
- **Performance benchmarks** on key changes

### Security Considerations

- **No secrets in code** - Use environment variables or config files
- **Validate all inputs** - Especially from network or user input
- **Follow Rust security best practices**
- **Regular dependency audits** via `cargo deny`

## Useful Makefile Targets

```bash
# Building
make install         # Install reth binary
make install-op      # Install op-reth binary
make build          # Build release binary
make build-debug    # Build debug binary

# Testing
make test-unit      # Unit tests only
make ef-tests       # Ethereum Foundation tests
make test          # All tests

# Quality
make fmt           # Format code
make clippy        # Run clippy
make lint          # All linting steps
make fix-lint      # Auto-fix linting issues

# Performance
make profiling     # Build with profiling symbols
make maxperf       # Maximum performance build

# Documentation
make rustdocs      # Generate Rust documentation
make update-book-cli  # Update CLI documentation

# Utilities
make clean         # Clean build artifacts
make db-tools      # Build MDBX debugging tools
```

## Common Patterns

### Error Handling

Use the error types in `crates/errors/` for consistency:

```rust
use reth_errors::{RethError, RethResult};

fn example_function() -> RethResult<T> {
    // ... implementation
}
```

### Metrics

Follow the patterns in `crates/metrics/`:

```rust
use reth_metrics::Metrics;

#[derive(Metrics)]
#[metrics(scope = "your_component")]
struct YourMetrics {
    /// Help text for the counter
    counter: Counter,
    /// Help text for the gauge  
    gauge: Gauge,
}
```

### Configuration

Use the config patterns from `crates/config/`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct YourConfig {
    /// Documentation for field
    pub field: Type,
}
```

## Getting Help

- **Documentation**: [Reth Book](https://paradigmxyz.github.io/reth/) and [Developer Docs](./docs/)
- **Telegram**: [Paradigm Reth](https://t.me/paradigm_reth) for development discussion
- **GitHub**: [Issues](https://github.com/paradigmxyz/reth/issues) and [Discussions](https://github.com/paradigmxyz/reth/discussions)
- **Code Examples**: Extensive examples in [`examples/`](./examples/)

## Contributing Guidelines

### Pull Request Process

1. **Fork the repository** or create a feature branch
2. **Follow this guide** for development setup and standards
3. **Write comprehensive tests** for your changes
4. **Update documentation** as needed
5. **Run the full quality pipeline** before submitting
6. **Create detailed PR description** explaining the changes

### Code Review Standards

- **Correctness** - Does the code work as intended?
- **Performance** - Are there any performance implications?
- **Security** - Are there security considerations?
- **Maintainability** - Is the code easy to understand and modify?
- **Testing** - Are there adequate tests?

### Commit Message Format

Use clear, descriptive commit messages:

```
feat: add new RPC method for transaction simulation

- Implements eth_simulateTransaction
- Includes comprehensive test coverage  
- Updates API documentation

Closes #1234
```

## License

Reth is dual-licensed under Apache 2.0 and MIT licenses. All contributions are made under these licenses.

---

*This guide is maintained by the Reth community. For updates or corrections, please open an issue or pull request.*
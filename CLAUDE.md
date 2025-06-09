# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## About Reth

Reth is a modular, high-performance Ethereum full node implementation written in Rust. It's production-ready since
v1.0 (June 2024) and supports Ethereum mainnet, testnets, and Optimism L2.

## Development Commands

### Building

```bash
make build                    # Release build of reth binary
make build-debug             # Debug build for development
make build-op                # Build op-reth (Optimism variant)
make install                 # Install reth to ~/.cargo/bin
```

### Testing

```bash
make test-unit               # Unit tests (uses cargo nextest - preferred)
make ef-tests                # Ethereum Foundation tests
make test                    # All tests (unit + doc tests)
make cov-unit                # Unit tests with coverage
```

### Code Quality (Required for PRs)

```bash
make lint                    # Format + clippy + spell check + TOML formatting
make fmt                     # Format code (requires nightly rustfmt)
make clippy                  # Run clippy lints
make fix-lint                # Auto-fix linting issues
make pr                      # Full PR check: lint + docs + tests
```

### Performance Builds

```bash
make profiling               # Optimized build with debug symbols
make maxperf                 # Maximum performance build
```

## Key Requirements

- **Rust MSRV:** 1.86.0
- **Required tools:** `cargo nextest`, `cargo hack`, `dprint`, `codespell`
- **Formatting:** Uses nightly rustfmt
- **Testing:** Always use `cargo nextest run` instead of `cargo test`

## Architecture Overview

Reth uses a modular, crate-based architecture inspired by Erigon's staged-sync design:

### Core Components

- **Storage** (`/crates/storage/`) - MDBX database, provider abstractions
- **Networking** (`/crates/net/`) - P2P, discovery, DevP2P, RLPx protocols
- **Consensus** (`/crates/consensus/`) - Block validation, proof verification
- **EVM** (`/crates/evm/`, `/crates/revm/`) - Transaction execution, state changes
- **Sync** (`/crates/stages/`) - Pipelined sync with staged architecture
- **RPC** (`/crates/rpc/`) - JSON-RPC with eth/debug/trace/engine APIs
- **Transaction Pool** (`/crates/transaction-pool/`) - Mempool management
- **Payload Building** (`/crates/payload/`) - Block construction and validation
- **Ethereum** (`/crates/ethereum/`) - Ethereum-specific logic, types, and implementations
- **Optimism** (`/crates/optimism/`) - Optimims(Opstack)-specific logic, types, and implementations

### Supporting Infrastructure

- **Primitives** (`/crates/primitives-traits/*`) - Core types, traits, constants
- **Node Builder** (`/crates/node/`) - Composable node construction
- **Optimism** (`/crates/optimism/`) - L2 rollup support (op-reth)

### Key Traits and Abstractions

- All components are designed as reusable libraries
- Core Abstractions:
    - `Block` trait for blocks of the chain. A block is composed of:
        - `Header` trait for block headers
        - `Body` trait for data within a block
    - `Body` is composed of transactions and other data
- Builder pattern for node configuration

## Development Workflow

1. **Setup:** Clone repo, ensure Rust 1.86.0+, install required tools
2. **Development:** Use `make build-debug` for faster iteration
3. **Testing:** Run `make test-unit` frequently, `make ef-tests` for full validation
4. **PR Preparation:** Always run `make pr` before submitting, which includes:
    - Linting (formatting, clippy, spell check)
    - Documentation checks
    - Full test suite (execessive, for simple changes use `make fix-lint` is sufficient)
5. **Open a PR:** PR title should follow conventional commits format, e.g. `feat: add new feature`,
   `fix: resolve issue #123`, include a description of changes and reference any related issues (e.g. `Closes #456`).

## Additional Notes

- Examples in `/examples/` show common customization patterns
- Each crate is designed to be used independently as a library
- Optimism support is first-class via separate `op-reth` binary
## Project Layout

This repository contains several Rust crates that implement the different building blocks of an Ethereum node. The high-level structure of the repository is as follows:

Generally, reth is composed of a few components, with supporting crates. The main components can be defined as:

- [Project Layout](#project-layout)
  - [Documentation](#documentation)
  - [Binaries](#binaries)
  - [CLI](#cli)
  - [Node](#node)
  - [Storage](#storage)
  - [Networking](#networking)
    - [Common](#common)
    - [Discovery](#discovery)
    - [Protocol](#protocol)
    - [Downloaders](#downloaders)
  - [Consensus](#consensus)
  - [Execution](#execution)
  - [Engine](#engine)
  - [Sync](#sync)
  - [RPC](#rpc)
    - [Transports](#transports)
    - [Common](#common-1)
    - [Utilities Crates](#utilities-crates)
  - [Payloads](#payloads)
  - [ExEx](#exex)
  - [Chain State](#chain-state)
  - [Pruning](#pruning)
  - [History](#history)
  - [Snap Sync](#snap-sync)
  - [Primitives](#primitives)
  - [Ethereum](#ethereum-specific-crates)
  - [Misc](#misc)

The supporting crates are split into two categories: [primitives](#primitives) and [miscellaneous](#misc).

### Documentation

Contributor documentation is in [`docs`](../../docs).

### Binaries

All binaries are stored in [`bin`](../../bin).

- [`bin/reth`](../../bin/reth): The main `reth` node binary.
- [`bin/reth-bb`](../../bin/reth-bb): A reth node configured for big-block payload execution (`reth-bb`).

### CLI

Crates implementing the `reth` command-line interface.

- [`cli/cli`](../../crates/cli/cli): Shared CLI abstractions used to build reth commands.
- [`cli/commands`](../../crates/cli/commands): Implementations of the `reth` subcommands (`db`, `download`, `import`, `p2p`, `stage`, ...).
- [`cli/runner`](../../crates/cli/runner): Runs CLI commands against a configured node.
- [`cli/util`](../../crates/cli/util): Small utilities shared by CLI commands (argument parsers, secret key loading, ...).

### Node

Crates for assembling, launching and running a node.

- [`node/api`](../../crates/node/api): Traits defining the node's components and their interfaces.
- [`node/builder`](../../crates/node/builder): Assembles and launches a node from its components.
- [`node/core`](../../crates/node/core): Core node types and CLI argument definitions shared across node implementations.
- [`node/types`](../../crates/node/types): Common node-related types.
- [`node/events`](../../crates/node/events): Node event listeners and logging.
- [`node/metrics`](../../crates/node/metrics): Metrics server, recorder and hooks.
- [`node/ethstats`](../../crates/node/ethstats): Optional ethstats reporting.

### Storage

These crates are related to the database.

- [`storage/codecs`](https://github.com/paradigmxyz/reth-core/tree/main/crates/codecs): Different storage codecs (lives in [reth-core](https://github.com/paradigmxyz/reth-core)).
- [`storage/codecs/derive`](https://github.com/paradigmxyz/reth-core/tree/main/crates/codecs-derive): Derive macros for storage codecs (lives in [reth-core](https://github.com/paradigmxyz/reth-core)).
- [`storage/libmdbx-rs`](../../crates/storage/libmdbx-rs): Rust bindings for [libmdbx](https://github.com/erthink/libmdbx). A fork of an earlier Apache-licensed version of [libmdbx-rs][libmdbx-rs].
  - [`storage/libmdbx-rs/mdbx-sys`](../../crates/storage/libmdbx-rs/mdbx-sys): Raw FFI bindings for libmdbx.
- [`storage/db`](../../crates/storage/db): Strongly typed Database abstractions (transactions, cursors, tables) over lower level database backends.
  - Implemented backends: mdbx
- [`storage/db-api`](../../crates/storage/db-api): High-level database access traits used across storage crates.
- [`storage/db-common`](../../crates/storage/db-common): Shared database helpers and utilities.
- [`storage/db-models`](../../crates/storage/db-models): Typed database models for on-disk tables.
- [`storage/storage-api`](../../crates/storage/storage-api): Storage provider traits and types used by higher-level components.
- [`storage/storage-overlay`](../../crates/storage/storage-overlay): State trie overlay construction and caching.
- [`storage/provider`](../../crates/storage/provider): Traits which provide a higher level api over the database to access the Ethereum state and historical data (transactions, blocks etc.)
- [`storage/rpc-provider`](../../crates/storage/rpc-provider): Storage provider implementations tailored for RPC access patterns.
- [`storage/errors`](../../crates/storage/errors): Common error types used by storage crates.
- [`storage/nippy-jar`](../../crates/storage/nippy-jar): Compressed columnar storage for historical data.
- [`storage/zstd-compressors`](https://github.com/paradigmxyz/reth-core/tree/main/crates/zstd-compressors): Zstandard-based compressors used by storage components (lives in [reth-core](https://github.com/paradigmxyz/reth-core)).


### Networking

These crates are related to networking (P2P).

The networking component mainly lives in [`net/network`](../../crates/net/network), which handles:

- Message egress/ingress
- Peer management
- Session management

#### Common

- [`net/banlist`](../../crates/net/banlist): A simple peer banlist that can be used to ban peers or IP addresses.
  - Contains: Peer banlist.
- [`net/network-api`](../../crates/net/network-api): Contains traits that define the networking component as a whole. Other components that interface with the network stack only need to depend on this crate for the relevant types.
- [`net/nat`](../../crates/net/nat): A small helper crate that resolves the external IP of the running node using various methods (such as a manually provided IP, using UPnP etc.)
- [`net/network-types`](../../crates/net/network-types): Common networking types (peer identifiers, capabilities, messages, etc.).
- [`net/p2p`](../../crates/net/p2p): Higher-level P2P networking helpers and utilities.
- [`net/peers`](../../crates/net/peers): Peer set management, scoring and reputation support.

#### Discovery

- [`net/discv4`](../../crates/net/discv4): An implementation of the [discv4][discv4] protocol
- [`net/discv5`](../../crates/net/discv5): An implementation of the discv5 node discovery protocol.
- [`net/dns`](../../crates/net/dns): An implementation of node discovery via DNS ([EIP-1459][eip-1459])

#### Protocol

- [`net/eth-wire`](../../crates/net/eth-wire): Implements the `eth` wire protocol and the ``RLPx`` networking stack.
- [`net/ecies`](../../crates/net/ecies): Implementation of the Elliptic Curve Integrated Encryption Scheme used in the ``RLPx`` handshake.
- [`net/eth-wire-types`](../../crates/net/eth-wire-types): Common types used by the `eth` wire protocol and RLPx networking stack.

#### Downloaders

- [`net/downloaders`](../../crates/net/downloaders): Traits defining block body and header downloaders, as well as P2P implementations of both.

### Consensus

Different consensus mechanisms.

- [`consensus/common`](../../crates/consensus/common): Common consensus functions and traits (e.g. fee calculation).
- [`consensus/consensus`](../../crates/consensus/consensus): Core consensus engine interfaces and implementations.
- [`consensus/debug-client`](../../crates/consensus/debug-client): Utilities for interacting with the consensus engine in debugging and testing scenarios.

### Execution

Crates related to transaction execution.

- [`revm`](../../crates/revm): Revm utils and implementations specific to reth.
- [`evm`](../../crates/evm/evm): Traits for configuring an EVM specifics.
- [`execution-types`](../../crates/evm/execution-types): Commonly used types for (EVM) block execution.
- [`execution-errors`](../../crates/evm/execution-errors): Commonly used error types used when doing block execution.

### Engine

The consensus engine: fork choice, payload processing and persistence.

- [`engine/tree`](../../crates/engine/tree): The execution engine's block tree, handling fork choice, payload processing and persistence.
- [`engine/primitives`](../../crates/engine/primitives): Engine API message types and primitives.
- [`engine/local`](../../crates/engine/local): A local (in-process) engine for testing and dev nodes.
- [`engine/util`](../../crates/engine/util): Utilities for engine payload processing.
- [`engine/execution-cache`](../../crates/engine/execution-cache): Cross-block execution cache for payload processing.
- [`engine/invalid-block-hooks`](../../crates/engine/invalid-block-hooks): Hooks for inspecting invalid blocks.

### Sync

These crates implement the main syncing drivers of reth.

- [`stages/api`](../../crates/stages/api): Public API for the staged sync pipeline.
- [`stages/stages`](../../crates/stages/stages): Implementations of the individual sync stages and the pipeline driver. This is used during initial sync and is faster than the tree-like structure for longer sync ranges.
- [`stages/types`](../../crates/stages/types): Shared types used by the staged sync pipeline.

### RPC

Crates related to the RPC component (including IPC transport)

The RPC component mainly lives in [`rpc/rpc`](../../crates/rpc/rpc), which implements the following namespaces:

- `admin_`
- `debug_` (includes Geth-style tracing APIs)
- `eth_`
- `net_`
- `trace_` (OpenEthereum-style tracing APIs)
- `txpool_`
- `web3_`

These RPC interface is defined in [`rpc/rpc-api`](../../crates/rpc/rpc-api).

The engine API ([`engine_`][engine-spec]) lives in [`rpc/rpc-engine-api`](../../crates/rpc/rpc-engine-api) (this is *not* an interface crate despite the confusing name).

There is also a crate to easily configure an RPC server: [`rpc/rpc-builder`](../../crates/rpc/rpc-builder).

#### Transports

The RPC component is based on the [`jsonrpsee`][jsonrpsee] crate which provides JSONRPC over WebSockets and HTTP.

The IPC transport lives in [`rpc/ipc`](../../crates/rpc/ipc).

#### Common

- [`rpc/rpc-api`](../../crates/rpc/rpc-api): RPC traits
  - Supported transports: HTTP, WS, IPC
  - Supported namespaces: `eth_`, `engine_`, `debug_`
- [`rpc/rpc-eth-api`](../../crates/rpc/rpc-eth-api/): Reth RPC 'eth' namespace API (including interface and implementation), this crate is re-exported by `rpc/rpc-api`
- [`rpc/rpc-eth-types`](../../crates/rpc/rpc-eth-types/): Types `supporting the implementation` of 'eth' namespace RPC server API
- [`rpc/rpc-server-types`](../../crates/rpc/rpc-server-types/): RPC server types and constants

#### Utilities Crates

- [`rpc/rpc-convert`](../../crates/rpc/rpc-convert): This crate provides various helper functions to convert between reth primitive types and rpc types.
- [`rpc/layer`](../../crates/rpc/rpc-layer/): Some RPC middleware layers (e.g. `AuthValidator`, `JwtAuthValidator`)
- [`rpc/rpc-e2e-tests`](../../crates/rpc/rpc-e2e-tests): RPC end-to-end tests, including execution-apis compatibility testing.

### Payloads

Crates related to building and validating payloads (blocks).

- [`transaction-pool`](../../crates/transaction-pool): An in-memory pending transactions pool.
- [`payload/builder`](../../crates/payload/builder): Abstractions for payload building and a payload builder service that works with multiple kinds of payload resolvers.
- [`payload/basic`](../../crates/payload/basic): A basic payload generator.
- [`payload/builder-primitives`](../../crates/payload/builder-primitives): Common primitives used by payload builders.
- [`payload/primitives`](../../crates/payload/primitives): Shared types used when building and validating payloads.
- [`payload/util`](../../crates/payload/util): Utility helpers used by payload building and validation logic.
- [`payload/validator`](../../crates/payload/validator): Payload validation helpers and utilities.

### ExEx

Execution Extensions (ExExes) let external components subscribe to the node's execution stream.

- [`exex/exex`](../../crates/exex/exex): The ExEx framework.
- [`exex/types`](../../crates/exex/types): Commonly used ExEx types.
- [`exex/test-utils`](../../crates/exex/test-utils): Test utilities for ExExes.

### Chain State

- [`chain-state`](../../crates/chain-state): In-memory canonical chain state and the notifications emitted when it changes.

### Pruning

- [`prune/prune`](../../crates/prune/prune): Pruning implementation.
- [`prune/types`](../../crates/prune/types): Commonly used types for prune usage in reth.

### History

Crates for importing and downloading pre-merge history in the ERA format.

- [`era`](../../crates/era): e2store and era1 file core logic.
- [`era-downloader`](../../crates/era-downloader): An asynchronous stream interface for downloading ERA files.
- [`era-utils`](../../crates/era-utils): Utilities to store and fetch history data with `storage-api`.

### Snap Sync

- [`snap-sync`](../../crates/snap-sync): snap/2 state synchronization.

### Primitives

These crates define primitive types or algorithms.

- [`chainspec`](../../crates/chainspec): Chain specifications and hardfork definitions.
- [`ethereum/primitives`](../../crates/ethereum/primitives): Commonly used Ethereum primitive types in Reth (successor to the removed top-level `crates/primitives`).
- [`primitives-traits`](https://github.com/paradigmxyz/reth-core/tree/main/crates/primitives-traits): Common abstracted types in reth (lives in [reth-core](https://github.com/paradigmxyz/reth-core)).
- [`trie/trie`](../../crates/trie/trie): An implementation of a Merkle Patricia Trie used for various roots (e.g. the state root) in Ethereum.
- [`trie/common`](../../crates/trie/common): Commonly used types for trie usage in reth.
- [`trie/db`](../../crates/trie/db): Database integration with the merkle trie implementation.
- [`trie/parallel`](../../crates/trie/parallel): Parallel implementation of the merkle root algorithm.
- [`trie/sparse`](../../crates/trie/sparse): Sparse MPT implementation.

### Ethereum-Specific Crates

Ethereum mainnet-specific implementations and primitives live in `crates/ethereum/`.

- **reth-ethereum** (`crates/ethereum/reth`)  
  The default Ethereum node implementation, wiring together the Ethereum-specific components.

- **reth-ethereum-consensus** (`crates/ethereum/consensus`)  
  Ethereum-specific consensus validation.

- **reth-ethereum-engine-primitives** (`crates/ethereum/engine-primitives`)  
  Ethereum-specific types for engine API, consensus messages, and payload attributes.

- **reth-ethereum-forks** (`crates/ethereum/hardforks`)  
  Mainnet-specific hardfork definitions, activation schedules, and feature flags.

- **reth-evm-ethereum** (`crates/ethereum/evm`)  
  Ethereum-specific EVM configuration and block executor.

- **reth-ethereum-payload-builder** (`crates/ethereum/payload`)  
  Ethereum-tailored payload builder implementing mainnet block production rules.

- **reth-node-ethereum** (`crates/ethereum/node`)  
  The Ethereum node type and its builder.

### Misc

Small utility crates.

- [`tasks`](../../crates/tasks): An executor-agnostic task abstraction, used to spawn tasks on different async executors. Supports blocking tasks and handles panics gracefully. A tokio implementation is provided by default.
- [`metrics/common`](../../crates/metrics/src/common): Common metrics types (e.g. metered channels)
- [`metrics`](https://github.com/metrics-rs/metrics): Reth use [metrics][metrics] for metrics collection, which is a lightweight metrics facade
- [`metrics/metrics-derive`](https://github.com/rkrasiuk/metrics-derive): A derive-style API for creating metrics
- [`metrics/reth-node-metrics`](../../crates/node/metrics/): The implementation of metrics server, recorder, hooks.
- [`tracing`](../../crates/tracing): A small utility crate to install a uniform [`tracing`][tracing] subscriber
- [`tracing-otlp`](../../crates/tracing-otlp): Exporter for sending [`tracing`][tracing] spans to OTLP/OTel backends.
- [`fs-util`](../../crates/fs-util): Small filesystem utilities shared across the node.
- [`tokio-util`](../../crates/tokio-util): Tokio-related utilities used by reth.
- [`static-file`](../../crates/static-file): Utilities for bundling and serving static files.
- [`errors`](../../crates/errors): Common error types shared across multiple crates.
- [`config`](../../crates/config): Node configuration types and loading.
- [`etl`](../../crates/etl): Extract-transform-load utilities for collecting and sorting large datasets on disk.
- [`dst`](../../crates/dst): Deterministic simulation testing (DST) support.
- [`e2e-test-utils`](../../crates/e2e-test-utils): Helpers for end-to-end tests of the node.
- [`testing/testing-utils`](../../testing/testing-utils): Testing utilities shared across reth crates.

[libmdbx-rs]: https://crates.io/crates/libmdbx
[discv4]: https://github.com/ethereum/devp2p/blob/master/discv4.md
[jsonrpsee]: https://github.com/paritytech/jsonrpsee/
[tracing]: https://crates.io/crates/tracing
[eip-1459]: https://eips.ethereum.org/EIPS/eip-1459
[engine-spec]: https://github.com/ethereum/execution-apis/tree/main/src/engine
[metrics]: https://github.com/metrics-rs/metrics

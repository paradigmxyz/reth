## Project Layout

This repository contains several Rust crates that implement the different building blocks of an Ethereum node. The high-level structure of the repository is as follows:

Generally, reth is composed of a few components, with supporting crates. The main components can be defined as:

- [Project Layout](#project-layout)
  - [Documentation](#documentation)
  - [Binaries](#binaries)
  - [Storage](#storage)
  - [Networking](#networking)
    - [Common](#common)
    - [Discovery](#discovery)
    - [Protocol](#protocol)
    - [Downloaders](#downloaders)
  - [Consensus](#consensus)
  - [Execution](#execution)
  - [Sync](#sync)
  - [RPC](#rpc)
    - [Transports](#transports)
    - [Common](#common-1)
    - [Utilities Crates](#utilities-crates)
  - [Payloads](#payloads)
  - [Primitives](#primitives)
  - [Ethereum](#ethereum-specific-crates)
  - [Misc](#misc)

The supporting crates are split into two categories: [primitives](#primitives) and [miscellaneous](#misc).

### Documentation

Contributor documentation is in [`docs`](../../docs).

### Binaries

All binaries are stored in [`bin`](../../bin).

### Storage

These crates are related to the database.

- [`storage/codecs`](../../crates/storage/codecs): Different storage codecs.
- [`storage/codecs/derive`](../../crates/storage/codecs/derive): Derive macros for storage codecs.
- [`storage/libmdbx-rs`](../../crates/storage/libmdbx-rs): Rust bindings for [libmdbx](https://github.com/erthink/libmdbx). A fork of an earlier Apache-licensed version of [libmdbx-rs][libmdbx-rs].
- [`storage/db`](../../crates/storage/db): Strongly typed Database abstractions (transactions, cursors, tables) over lower level database backends.
  - Implemented backends: mdbx
- [`storage/db-api`](../../crates/storage/db-api): High-level database access traits used across storage crates.
- [`storage/db-common`](../../crates/storage/db-common): Shared database helpers and utilities.
- [`storage/db-models`](../../crates/storage/db-models): Typed database models for on-disk tables.
- [`storage/storage-api`](../../crates/storage/storage-api): Storage-facing APIs used by higher-level components.
- [`storage/provider`](../../crates/storage/provider): Traits which provide a higher level api over the database to access the Ethereum state and historical data (transactions, blocks etc.)
- [`storage/rpc-provider`](../../crates/storage/rpc-provider): Storage provider implementations tailored for RPC access patterns.
- [`storage/errors`](../../crates/storage/errors): Common error types used by storage crates.
- [`storage/nippy-jar`](../../crates/storage/nippy-jar): Compressed columnar storage for historical data.
- [`storage/zstd-compressors`](../../crates/storage/zstd-compressors): Zstandard-based compressors used by storage components.


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
- [`rpc/rpc-testing-util`](../../crates/rpc/rpc-testing-util/): Reth RPC testing helpers

### Payloads

Crates related to building and validating payloads (blocks).

- [`transaction-pool`](../../crates/transaction-pool): An in-memory pending transactions pool.
- [`payload/builder`](../../crates/payload/builder): Abstractions for payload building and a payload builder service that works with multiple kinds of payload resolvers.
- [`payload/basic`](../../crates/payload/basic): A basic payload generator.
- [`payload/builder-primitives`](../../crates/payload/builder-primitives): Common primitives used by payload builders.
- [`payload/primitives`](../../crates/payload/primitives): Shared types used when building and validating payloads.
- [`payload/util`](../../crates/payload/util): Utility helpers used by payload building and validation logic.
- [`payload/validator`](../../crates/payload/validator): Payload validation helpers and utilities.

### Primitives

These crates define primitive types or algorithms.

- [`primitives`](../../crates/primitives): Commonly used types in Reth.
- [`primitives-traits`](../../crates/primitives-traits/): Common abstracted types in reth.
- [`trie`](../../crates/trie): An implementation of a Merkle Patricia Trie used for various roots (e.g. the state root) in Ethereum.

### Ethereum-Specific Crates

Ethereum mainnet-specific implementations and primitives live in `crates/ethereum/`.

- **reth-ethereum-engine-primitives** (`crates/ethereum/engine-primitives`)  
  Ethereum-specific types for engine API, consensus messages, and payload attributes.

- **reth-ethereum-forks** (`crates/ethereum/hardforks`)  
  Mainnet-specific hardfork definitions, activation schedules, and feature flags.

- **reth-ethereum-payload-builder** (`crates/ethereum/payload`)  
  Ethereum-tailored payload builder implementing mainnet block production rules.

### Misc

Small utility crates.

- [`tasks`](../../crates/tasks): An executor-agnostic task abstraction, used to spawn tasks on different async executors. Supports blocking tasks and handles panics gracefully. A tokio implementation is provided by default.
- [`metrics/common`](../../crates/metrics/src/common): Common metrics types (e.g. metered channels)
- [`metrics`](https://github.com/metrics-rs/metrics): Reth use [metrics][metrics] for metrics collection, which is a lightweight metrics facade
- [`metrics/metrics-derive`](https://github.com/rkrasiuk/metrics-derive): A derive-style API for creating metrics
- [`metrics/reth-node-metrics`](../../crates/node/metrics/): The implementation of metrics server, recorder, hooks.
- [`tracing`](../../crates/tracing): A small utility crate to install a uniform [`tracing`][tracing] subscriber
- [`fs-util`](../../crates/fs-util): Small filesystem utilities shared across the node.
- [`tokio-util`](../../crates/tokio-util): Tokio-related utilities used by reth.
- [`static-file`](../../crates/static-file): Utilities for bundling and serving static files.
- [`tracing-otlp`](../../crates/tracing-otlp): Exporter for sending [`tracing`][tracing] spans to OTLP/OTel backends.
- [`errors`](../../crates/errors): Common error types shared across multiple crates.
- [`e2e-test-utils`](../../crates/e2e-test-utils): Helpers for end-to-end tests of the node.

[libmdbx-rs]: https://crates.io/crates/libmdbx
[discv4]: https://github.com/ethereum/devp2p/blob/master/discv4.md
[jsonrpsee]: https://github.com/paritytech/jsonrpsee/
[tracing]: https://crates.io/crates/tracing
[eip-1459]: https://eips.ethereum.org/EIPS/eip-1459
[engine-spec]: https://github.com/ethereum/execution-apis/tree/main/src/engine
[metrics]: https://github.com/metrics-rs/metrics

## Project Layout

This repository contains several Rust crates that implement the different building blocks of an Ethereum node. The high-level structure of the repository is as follows:

### Documentation

Contributor documentation is in [`docs`](../../docs) and end-user documentation is in [`book`](../../book).

### Binaries

All binaries are stored in [`bin`](../../bin).

### Primitives

These crates define primitive types or algorithms such as RLP.

- [`primitives`](../../crates/primitives): Commonly used types in Reth.
- [`rlp`](../../crates/rlp): An implementation of RLP, forked from an earlier Apache-licensed version of [`fastrlp`][fastrlp]
- [`rlp/rlp-derive`](../../crates/rlp/rlp-derive): Forked from an earlier Apache licenced version of the [`fastrlp-derive`][fastrlp-derive] crate, before it changed licence to GPL.

### Storage

These crates are related to the database.

- [`storage/codecs`](../../crates/storage/codecs): Different storage codecs.
- [`storage/libmdbx-rs`](../../crates/storage/libmdbx-rs): Rust bindings for [libmdbx](https://libmdbx.dqdkfa.ru). A fork of an earlier Apache-licensed version of [libmdbx-rs][libmdbx-rs].
- [`storage/db`](../../crates/storage/db): Strongly typed Database abstractions (transactions, cursors, tables) over lower level database backends.
  - Implemented backends: mdbx
- [`storage/provider`](../../crates/storage/provider): Traits which provide a higher level api over the database to access the Ethereum state and historical data (transactions, blocks etc.)

### Networking

These crates are related to networking (p2p and RPC), as well as networking protocols.

- [`net/common`](../../crates/net/common): Shared types used across multiple networking crates.
  - Contains: Peer banlist.


#### P2P

- [`net/network`](../../crates/net/network): The main P2P networking crate, handling message egress, message ingress, peer management, and session management.
- [`net/eth-wire`](../../crates/net/eth-wire): Implements the `eth` wire protocol and the RLPx networking stack.
- [`net/discv4`](../../crates/net/discv4): An implementation of the [discv4][discv4] protocol
- [`net/ipc`](../../crates/net/ipc): IPC server and client implementation for [`jsonrpsee`][jsonrpsee].
- [`net/ecies`](../../crates/net/ecies): Implementation of the Elliptic Curve Integrated Encryption Scheme used in the RLPx handshake.

#### RPC

- [`net/rpc-api`](../../crates/rpc/rpc-api): RPC traits
  - Supported transports: HTTP, WS, IPC
  - Supported namespaces: `eth_`, `engine_`, `debug_`
- [`net/rpc`](../../crates/rpc/rpc): Implementation of all ETH JSON RPC traits defined in `rpc-api`.
- [`net/rpc-types`](../../crates/rpc/rpc-types): Types relevant for the RPC endpoints above, grouped by namespace

#### Downloaders

- [`net/downloaders`](../../crates/net/downloaders/): Block body and header downloading strategies.

### Ethereum

These crates are Ethereum-specific (e.g. EVM, consensus, transaction pools).

- [`executor`](../../crates/executor): Blazing-fast instrumented EVM using [`revm`](https://github.com/bluealloy/revm/). Used during consensus, syncing & during transaction simulation / gas estimation.
- [`consensus`](../../crates/consensus): Implementations of consensus protocols.
- [`transaction-pool`](../../crates/transaction-pool): An in-memory pending transactions pool.

### Staged sync

These crates are related to staged sync.

- [`stages`](../../crates/stages): The staged sync pipeline, including implementations of each stage.

### Misc

Small utility crates.

- [`interfaces`](../../crates/interfaces): Traits containing common abstractions across the components used in the system. For ease of unit testing, each crate importing the interface is recommended to create mock/in-memory implementations of each trait.
- [`tracing`](../../crates/tracing): A small utility crate to install a uniform [`tracing`][tracing] subscriber
- [`crate-template`](../../crate-template): Template crate to use when instantiating new crates under `crates/`.

[fastrlp]: https://crates.io/crates/fastrlp
[fastrlp-derive]: https://crates.io/crates/fastrlp-derive
[libmdbx-rs]: https://crates.io/crates/libmdbx
[discv4]: https://github.com/ethereum/devp2p/blob/master/discv4.md
[jsonrpsee]: https://github.com/paritytech/jsonrpsee/
[tracing]: https://crates.io/crates/tracing

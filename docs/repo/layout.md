## Project Layout

This repository contains several Rust crates that implement the different building blocks of an Ethereum node. The high-level structure of the repository is as follows:

- `crates/`
    - [`db`](../../crates/db): Strongly typed database bindings to [LibMDBX](https://github.com/vorot93/libmdbx-rs/) containing read/write access to Ethereum state and historical data (transactions, blocks etc.)
    - [`executor`](../../crates/executor): Blazing-fast instrumented EVM using [`revm`](https://github.com/bluealloy/revm/). Used during consensus, syncing & during transaction simulation / gas estimation.
    - [`interfaces`](../../crates/interfaces): Traits containing common abstractions across the components used in the system. For ease of unit testing, each crate importing the interface is recommended to create mock/in-memory implementations of each trait.
    - [`net/p2p`](../../crates/net/p2p): Implements the Ethereum P2P protocol.
    - [`net/rpc-api`](../../crates/net/rpc-api): Traits
        - Supported transports: HTTP, WS, IPC
        - Supported namespaces: `eth_`, `engine_`, `debug_`
    - [`net/rpc`](../../crates/net/rpc): Implementation of all ETH JSON RPC traits defined in `rpc-api`.
    - [`net/rpc-types`](../../crates/net/rpc-types): Types relevant for the RPC endpoints above, grouped by namespace
    - [`primitives`](../../crates/stages): Commonly used types in Reth.
    - [`stages`](../../crates/stages): The staged sync pipeline, including implementations of each stage.
    - [`transaction-pool`](../../crates/transaction-pool): An in-memory pending transactions pool.
- [`crate-template`](../../crate-template): Template crate to use when instantiating new crates under `crates/`.
- [`bin`](../../bin): Where all binaries are stored.
- [`examples`](../../examples): Example usage of the reth stack as a library.

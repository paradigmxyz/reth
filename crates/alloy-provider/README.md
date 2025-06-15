# Alloy Provider for Reth

This crate provides an implementation of reth's `StateProviderFactory` and related traits that fetches state data via RPC instead of from a local database.

Originally created by [cakevm](https://github.com/cakevm/alloy-reth-provider).

## Features

- Implements `StateProviderFactory` for remote RPC state access
- Supports Ethereum networks
- Useful for testing without requiring a full database
- Can be used with reth ExEx (Execution Extensions) for testing

## Usage

```rust
use alloy_provider::ProviderBuilder;
use reth_alloy_provider::AlloyRethProvider;
use reth_ethereum_node::EthereumNode;

// Initialize provider
let provider = ProviderBuilder::new()
    .builtin("https://eth.merkle.io")
    .await
    .unwrap();

// Create database provider with NodeTypes
let db_provider = AlloyRethProvider::new(provider, EthereumNode);

// Get state at specific block
let state = db_provider.state_by_block_id(BlockId::number(16148323)).unwrap();
```

## Configuration

The provider can be configured with custom settings:

```rust
use reth_alloy_provider::{AlloyRethProvider, AlloyRethProviderConfig};
use reth_ethereum_node::EthereumNode;

let config = AlloyRethProviderConfig {
    compute_state_root: true, // Enable state root computation
};

let db_provider = AlloyRethProvider::new_with_config(provider, EthereumNode, config);
```

## Technical Details

The provider uses `alloy_network::AnyNetwork` for network operations, providing compatibility with various Ethereum-based networks while maintaining the expected block structure with headers.

## License

Licensed under either of:

- Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.
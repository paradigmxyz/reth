# RPC Blockchain Provider for Reth

This crate provides an RPC-based implementation of reth's [`BlockchainProvider`](../provider/src/providers/blockchain_provider.rs) which provides access to local blockchain data, this crate offers the same functionality but for remote blockchain access via RPC.

Originally created by [cakevm](https://github.com/cakevm/alloy-reth-provider).

## Features

- Provides the same interface as `BlockchainProvider` but for remote nodes
- Implements `StateProviderFactory` for remote RPC state access
- Supports Ethereum networks
- Useful for testing without requiring a full database
- Can be used with reth ExEx (Execution Extensions) for testing

## Usage

```rust
use alloy_provider::ProviderBuilder;
use reth_storage_rpc_provider::RpcBlockchainProvider;

// Initialize provider
let provider = ProviderBuilder::new()
    .builtin("https://eth.merkle.io")
    .await
    .unwrap();

// Create RPC blockchain provider with NodeTypes
let rpc_provider = RpcBlockchainProvider::new(provider);

// Get state at specific block - same interface as BlockchainProvider
let state = rpc_provider.state_by_block_id(BlockId::number(16148323)).unwrap();
```

## Configuration

The provider can be configured with custom settings:

```rust
use reth_storage_rpc_provider::{RpcBlockchainProvider, RpcBlockchainProviderConfig};

let config = RpcBlockchainProviderConfig {
    compute_state_root: true,  // Enable state root computation
    reth_rpc_support: true,    // Use Reth-specific RPC methods (default: true)
};

let rpc_provider = RpcBlockchainProvider::new_with_config(provider, config);
```

## Configuration Options

- `compute_state_root`: When enabled, computes state root and trie updates (requires Reth-specific RPC methods)
- `reth_rpc_support`: When enabled (default), uses Reth-specific RPC methods for better performance:
  - `eth_getAccountInfo`: Fetches account balance, nonce, and code in a single call
  - `debug_codeByHash`: Retrieves bytecode by hash without needing the address
  
  When disabled, falls back to standard RPC methods and caches bytecode locally for compatibility with non-Reth nodes.

## Technical Details

The `RpcBlockchainProvider` uses `alloy_network::AnyNetwork` for network operations, providing compatibility with various Ethereum-based networks while maintaining the expected block structure with headers.

This provider implements the same traits as the local `BlockchainProvider`, making it a drop-in replacement for scenarios where remote RPC access is preferred over local database access.

## License

Licensed under either of:

- Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

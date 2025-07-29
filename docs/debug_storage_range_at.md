# debug_storageRangeAt Implementation

## Overview

This document describes the implementation of the `debug_storageRangeAt` RPC method for Reth, which allows clients to retrieve storage entries for a specific contract at a given block and transaction index.

## Method Signature

```rust
async fn debug_storage_range_at(
    &self,
    block_hash: B256,
    tx_idx: usize,
    contract_address: Address,
    key_start: B256,
    max_result: u64,
) -> RpcResult<StorageRangeResponse>
```

## Parameters

- `block_hash`: The hash of the block to query
- `tx_idx`: The transaction index within the block (0-based)
- `contract_address`: The address of the contract whose storage to query
- `key_start`: The starting storage key for pagination
- `max_result`: Maximum number of storage entries to return

## Response Type

```rust
pub struct StorageRangeResponse {
    /// Storage entries in the range
    pub storage: HashMap<B256, StorageEntry>,
    /// Next key to continue pagination, if any
    pub next_key: Option<B256>,
}

pub struct StorageEntry {
    /// Storage key
    pub key: B256,
    /// Storage value
    pub value: U256,
}
```

## Implementation Details

### Current Implementation

The current implementation provides a basic framework for the `debug_storageRangeAt` method:

1. **Block Validation**: Verifies that the specified block exists
2. **Transaction Index Validation**: Ensures the transaction index is valid for the block
3. **State Access**: Retrieves the state provider for the specified block
4. **Storage Retrieval**: Attempts to retrieve storage values for the contract

### Simplified Storage Iteration

The current implementation uses a simplified approach to retrieve storage entries:

- Iterates through a limited number of storage keys starting from `key_start`
- Only includes non-zero storage values
- Respects the `max_result` limit for pagination
- Sets `next_key` when more results are available

### Future Improvements

The implementation can be enhanced with:

1. **Proper Storage Cursor**: Use the database storage cursor to iterate through actual storage entries
2. **Key Range Support**: Implement proper key range iteration starting from `key_start`
3. **Storage Trie Navigation**: Navigate the storage trie to find all storage entries for the contract
4. **Performance Optimization**: Implement efficient storage iteration with proper indexing

## Usage Example

```json
{
  "method": "debug_storageRangeAt",
  "params": [
    "0x1234567890abcdef...",
    0,
    "0x742d35Cc6634C0532925a3b8D4C9db96C4b4d8b6",
    "0x0000000000000000000000000000000000000000000000000000000000000000",
    100
  ]
}
```

## Response Example

```json
{
  "storage": {
    "0x0000000000000000000000000000000000000000000000000000000000000001": {
      "key": "0x0000000000000000000000000000000000000000000000000000000000000001",
      "value": "0x0000000000000000000000000000000000000000000000000000000000000001"
    }
  },
  "next_key": "0x0000000000000000000000000000000000000000000000000000000000000002"
}
```

## Compatibility

This implementation follows the Geth-style debug API pattern and is compatible with existing Ethereum tooling that expects the `debug_storageRangeAt` method.

## Testing

The implementation includes basic serialization tests to ensure the response types work correctly with JSON-RPC serialization. 
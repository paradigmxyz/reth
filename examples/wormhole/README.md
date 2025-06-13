# Wormhole Node Example

This is a demonstration of how to create a custom Optimism node with extended transaction types using the Reth framework.

## Overview

This example shows how to:

1. **Define Custom Transaction Types**: The `WormholeTransaction` enum extends Optimism transactions with support for an additional transaction type
2. **Implement Required Traits**: All necessary traits are implemented for the custom transaction types including:
   - `Transaction` - Core transaction interface
   - `SignedTransaction` - For signed transaction handling  
   - `SignerRecoverable` - For address recovery from signatures
   - `Encodable2718`/`Decodable2718` - For EIP-2718 typed transaction encoding
   - `SerdeBincodeCompat` - For efficient serialization
   - Serde traits for JSON serialization
3. **Custom Node Primitives**: Defines `WormholeNodePrimitives` that specify the custom transaction types
4. **Transaction Pool Integration**: Shows how to configure transaction pools with custom transaction types

## Key Components

### Transaction Types (`primitives/tx.rs`)

- `WormholeTransaction`: An enum that wraps both Optimism transactions and placeholder Wormhole transactions
- `WormholeTransactionSigned`: A signed version that caches the transaction hash
- Custom transaction type ID: `0x7E` (126) for Wormhole transactions

### Block Types (`primitives/block.rs`)

- `WormholeBlock`: Block type using the custom transactions
- `WormholeBlockBody`: Block body with custom transaction list
- Various sealed and recovered block variants

### Pool Integration (`pool.rs`)

- `WormholePooledTransaction`: Transaction pool type supporting custom transactions
- Integration with Optimism's transaction validation

## Implementation Notes

**For Demonstration**: This implementation uses `OpTransactionSigned` for both transaction variants to ensure compatibility with existing Reth infrastructure. In a production implementation, you would:

1. Implement actual Wormhole transaction logic
2. Add proper transaction validation for Wormhole-specific fields
3. Implement custom EVM execution for Wormhole opcodes
4. Add network protocol support for the new transaction types
5. Update consensus logic to handle the new transaction types

## Recent Fixes

- **Corrected SignerRecoverable return types**: Fixed the trait implementations to return `Result<Address, RecoveryError>` instead of `Option<Address>`
- **Cleaned up dependencies**: Removed unused dependencies and kept only essential ones for the demo
- **Fixed serde-bincode-compat**: Properly implemented the `SerdeBincodeCompat` trait which is required for Reth's serialization

## Architecture

The example follows Reth's modular architecture:

```
WormholeNode
├── Primitives (WormholeNodePrimitives)
│   ├── Block Types
│   ├── Transaction Types (WormholeTransaction)
│   └── Receipt Types
├── Transaction Pool (supports custom transactions)
└── Node Components (reuses Optimism components)
```

## Usage

This example demonstrates the structure needed for custom transaction support in Reth. To build upon this:

1. Replace the placeholder Wormhole transaction logic with actual implementation
2. Add custom EVM execution logic for new transaction types
3. Implement custom validation rules
4. Add network support for broadcasting custom transactions
5. Update consensus mechanisms as needed

## Building

```bash
cargo check -p wormhole
cargo test -p wormhole
```

The example compiles successfully and demonstrates all the key integration points needed for custom transaction types in Reth.
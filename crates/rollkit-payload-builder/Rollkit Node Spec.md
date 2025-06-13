# Rollkit Reth Node Specification

## Overview
This specification outlines the implementation of a custom payload builder for a Reth node used by Rollkit that accepts transactions via payload attributes. The payload builder will execute provided transactions instead of building payloads from the mempool.

## Architecture

### 1. Project Structure
```
reth/
├── crates/
│   └── custom-payload-builder/
│       ├── src/
│       │   ├── main.rs
│       │   ├── builder.rs
│       │   ├── config.rs
│       │   ├── attributes.rs
│       │   └── types.rs
│       └── Cargo.toml
```

### 2. Components

#### 2.1 Custom Payload Attributes
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomPayloadAttributes {
    /// List of transactions to be executed in the payload
    pub transactions: Vec<Transaction>,
    /// Optional gas limit for the transactions
    pub gas_limit: Option<u64>,
    // ... other standard payload attributes
}
```

#### 2.2 Custom Payload Builder
- Implements the `PayloadServiceBuilder` trait
- Handles transaction execution from payload attributes
- Manages payload building lifecycle
- Respects gas limits for transaction lists

#### 2.3 Configuration
- Custom configuration for the payload builder
- Transaction execution parameters
- Block building constraints

## Implementation Steps

### 1. Create New Crate
```bash
cargo new --lib crates/custom-payload-builder
```

### 2. Dependencies
Add the following to `Cargo.toml`:
```toml
[dependencies]
reth-basic-payload-builder = { path = "../basic-payload-builder" }
reth-payload-builder = { path = "../payload-builder" }
reth-ethereum = { path = "../ethereum" }
reth-primitives = { path = "../primitives" }
reth-provider = { path = "../provider" }
reth-tasks = { path = "../tasks" }
```

### 3. Implementation Details

#### 3.1 Custom Payload Attributes
- Create `CustomPayloadAttributes` struct
- Implement serialization/deserialization
- Add validation methods for transactions and gas limits

#### 3.2 Custom Payload Builder
- Create a new struct implementing `PayloadServiceBuilder`
- Override the `spawn_payload_builder_service` method
- Implement transaction execution logic
- Handle gas limit constraints

#### 3.3 Transaction Execution
- Accept transactions from payload attributes
- Validate transactions against gas limits
- Execute transactions in the specified order
- Build payload with executed transactions

#### 3.4 Block Building
- Implement block building logic
- Handle block finalization
- Manage block state updates

### 4. Integration Steps

1. Create the custom payload builder crate
2. Implement the payload attributes
3. Implement the payload builder service
4. Create configuration types
5. Implement transaction execution logic
6. Add block building functionality
7. Integrate with the node builder API
8. Test the implementation

## Payload Building Flow

1. Receive payload attributes with transactions and gas limit
2. Validate transactions and gas limit
3. Execute transactions in order:
   - Check gas limit constraints
   - Execute transaction
   - Update cumulative gas used
4. Build block with executed transactions
5. Return built payload

## Testing

### 1. Unit Tests
- Test payload attributes validation
- Test transaction execution
- Test gas limit handling
- Test block building
- Test configuration handling

### 2. Integration Tests
- Test payload builder integration
- Test node builder integration
- Test end-to-end flow

### 3. Performance Tests
- Measure transaction execution time
- Measure block building time
- Measure memory usage

## Security Considerations

1. Transaction validation
2. Gas limit validation
3. Block validation
4. State management
5. Error handling
6. Resource limits

## Next Steps

1. Create the basic crate structure
2. Implement payload attributes
3. Implement core functionality
4. Add tests
5. Document the implementation
6. Create example usage
7. Performance optimization

## References

- [reth Node Builder API](https://www.paradigm.xyz/2024/04/reth-alphanet)
- [reth Examples](https://github.com/paradigmxyz/reth/tree/main/examples)
- [Optimism Payload Builder](https://github.com/paradigmxyz/reth/tree/main/crates/optimism/payload) 
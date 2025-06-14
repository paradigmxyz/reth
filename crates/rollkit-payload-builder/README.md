# Rollkit Payload Builder

A custom payload builder for Reth that integrates with Rollkit, supporting transaction execution via the Engine API's `engine_forkchoiceUpdatedV3` method.

## Overview

The Rollkit Payload Builder extends Reth to support custom payload building patterns similar to Optimism's sequencer transactions. It allows transactions to be passed directly through the Engine API, enabling integration with rollup sequencers and other Layer 2 solutions.

## Quick Start

```bash
# 1. Build the rollkit-reth binary
cargo build --bin rollkit-reth

# 2. Run the rollkit node with basic configuration
./target/debug/rollkit-reth node --rollkit --chain dev --http

# 3. The node will start with rollkit-specific Engine API support
# and accept transactions via engine_forkchoiceUpdatedV3
```

## Key Features

- **Engine API Integration**: Supports `engine_forkchoiceUpdatedV3` with transaction passing
- **Custom Payload Attributes**: Extends standard Ethereum payload attributes with transaction data
- **Gas Limit Management**: Configurable gas limits with transaction filtering
- **Rollkit Integration**: Native integration with the Rollkit consensus mechanism
- **Comprehensive Validation**: Custom validators for rollkit-specific requirements

## Architecture

The implementation follows the custom-engine-types pattern from Reth, consisting of:

1. **RollkitEnginePayloadAttributes**: Custom payload attributes supporting transaction arrays
2. **RollkitEnginePayloadBuilderAttributes**: Builder attributes for processing Engine API data
3. **RollkitEngineTypes**: Custom engine types implementing PayloadTypes and EngineTypes
4. **RollkitEngineValidator**: Custom validator for rollkit-specific validation rules
5. **RollkitNode**: Complete node implementation with all components

## Building and Usage

### Building the Rollkit Node

The rollkit node is built as a standard reth binary with rollkit-specific functionality:

```bash
# Build in debug mode (faster compilation, for development)
cargo build --bin rollkit-reth

# Build in release mode (optimized, for production)
cargo build --release --bin rollkit-reth
```

The binary will be created in:
- Debug mode: `target/debug/rollkit-reth`
- Release mode: `target/release/rollkit-reth`

### Running the Rollkit Node

The `rollkit-reth` binary supports all standard reth CLI commands and flags, plus rollkit-specific extensions:

```bash
# Show help and available commands
./target/debug/rollkit-reth --help

# Show node-specific options (including rollkit flags)
./target/debug/rollkit-reth node --help

# Run basic rollkit node with default settings
./target/debug/rollkit-reth node --rollkit

# Run with custom rollkit configuration
./target/debug/rollkit-reth node \
  --rollkit \
  --rollkit-gas-limit 50000000 \
  --engine-tx-passthrough \
  --http \
  --http.api eth,engine,net

# Run with full logging and custom chain
RUST_LOG=debug ./target/debug/rollkit-reth node \
  --chain dev \
  --rollkit \
  --rollkit-gas-limit 30000000 \
  --http \
  --ws
```

### Rollkit-Specific CLI Flags

The rollkit node adds these additional flags to the standard reth CLI:

- `--rollkit`: Enable rollkit mode
- `--rollkit-gas-limit <GAS_LIMIT>`: Maximum gas limit for rollkit payloads (default: 30000000)
- `--engine-tx-passthrough`: Enable transaction passthrough via Engine API (default: true)

### Standard Reth Commands

All standard reth commands work with the rollkit node:

```bash
# Initialize database
./target/debug/rollkit-reth init --chain dev

# Database debugging
./target/debug/rollkit-reth db stats

# Import blocks
./target/debug/rollkit-reth import <file>

# Configuration management
./target/debug/rollkit-reth config
```

### Running Engine API Tests

```bash
# Run the comprehensive Engine API test suite
cargo run --example test_rollkit_engine_api

# Run specific integration tests
cargo test --example test_rollkit_engine_api
```

## Engine API Support

The rollkit node supports the following Engine API methods with transaction passing:

### engine_forkchoiceUpdatedV3

Payload attributes can include a `transactions` field with raw transaction bytes:

```json
{
  "timestamp": "0x12345678",
  "prevRandao": "0x...",
  "suggestedFeeRecipient": "0x...",
  "withdrawals": [],
  "parentBeaconBlockRoot": "0x...",
  "transactions": [
    "0x02f86e01808459682f008459682f0e82520894...",
    "0x02f86e01018459682f008459682f0e82520894..."
  ],
  "gasLimit": "0x1c9c380"
}
```

### Supported Workflow

1. **Payload Building**: Call `engine_forkchoiceUpdatedV3` with transaction data
2. **Payload Retrieval**: Use returned payload ID with `engine_getPayloadV3`
3. **Payload Submission**: Submit built payload via `engine_newPayloadV3`
4. **Chain Progression**: Continue with standard Engine API flow

## Configuration

The rollkit payload builder can be configured with:

```rust
let config = RollkitPayloadBuilderConfig {
    max_transactions: 1000,
    max_gas_limit: 30_000_000,
    min_gas_price: 1_000_000_000,
    enable_tx_validation: true,
};
```

## Integration Examples

### Basic Integration

```rust
use rollkit_payload_builder::*;

// Create rollkit node
let node = RollkitNode::default();

// Build and launch
let handle = NodeBuilder::new(config)
    .testing_node(tasks.executor())
    .launch_node(node)
    .await?;
```

### Custom Payload Builder

```rust
// Create custom payload builder with configuration
let builder = RollkitPayloadBuilderBuilder::with_config(config);

// Integrate with node components
let components = ComponentsBuilder::default()
    .payload(BasicPayloadServiceBuilder::new(builder))
    .build();
```

## Testing

The test suite includes:

### Unit Tests
- Payload attributes validation
- Transaction decoding and encoding
- Gas limit enforcement
- Configuration validation

### Integration Tests
- Full node startup and shutdown
- Engine API connectivity
- Transaction processing pipeline
- Payload lifecycle management

### Engine API Tests
- `engine_forkchoiceUpdatedV3` with transactions
- Payload retrieval and validation
- Gas limit boundary testing
- Full payload lifecycle testing

Run all tests:

```bash
cargo test
```

Run specific test categories:

```bash
# Unit tests only
cargo test --lib

# Integration tests
cargo test --test integration

# Engine API tests
cargo run --example test_rollkit_engine_api
```

## Development

### Building

```bash
# Build the rollkit-reth binary
cargo build --bin rollkit-reth

# Build with release optimizations
cargo build --release --bin rollkit-reth

# Build library for development
cargo build
```

### Running with Debug Logging

```bash
# Run with detailed logging
RUST_LOG=debug,rollkit_payload_builder=trace ./target/debug/rollkit-reth node --rollkit

# Run with trace-level logging for rollkit components
RUST_LOG=trace ./target/debug/rollkit-reth node --rollkit --chain dev --http

# Run examples with debug logging
RUST_LOG=debug cargo run --example rollkit_node
```

### Adding Custom Validation

Extend the `RollkitEngineValidator` to add custom validation logic:

```rust
impl<T> EngineValidator<T> for RollkitEngineValidator {
    fn ensure_well_formed_attributes(&self, attrs: &T::PayloadAttributes) -> Result<()> {
        // Add custom validation here
        validate_custom_rollkit_rules(attrs)?;
        Ok(())
    }
}
```

## Troubleshooting

### Common Issues

1. **Transaction Decoding Errors**: Ensure transactions are properly RLP-encoded
2. **Gas Limit Exceeded**: Check transaction gas requirements vs. payload gas limit
3. **Engine API Connectivity**: Verify RPC server is running and accessible
4. **Validation Failures**: Check custom validation rules and transaction format

### Debug Mode

Enable detailed logging:

```bash
RUST_LOG=trace cargo run --example rollkit_node 2>&1 | grep rollkit
```

## Contributing

1. Fork the repository
2. Create a feature branch
3. Add tests for new functionality
4. Ensure all tests pass
5. Submit a pull request

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your option. 
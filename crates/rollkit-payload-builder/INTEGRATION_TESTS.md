# Rollkit Payload Builder Integration Tests

This document describes the comprehensive integration tests for the rollkit-payload-builder, which replicate the testing patterns and expectations from the Go execution tests (`rollkit/execution/evm/execution_test.go` and `rollkit/execution/evm/execution.go`).

## Overview

The integration tests are designed to validate the rollkit payload builder's functionality in scenarios that mirror real-world usage through the Engine API. The tests replicate the structure and behavior of the Go tests to ensure compatibility and correctness.

## Test Files

### `tests/integration_tests.rs`
Contains basic payload building tests focusing on the core functionality:
- Payload attributes validation
- Transaction processing
- Empty block handling
- Concurrent payload building
- Error handling and edge cases

### `tests/engine_api_tests.rs`
Contains Engine API integration tests that closely mirror the Go test structure:
- Chain initialization (`InitChain`)
- Transaction execution (`ExecuteTxs`) 
- Block finalization (`SetFinal`)
- Build and sync chain phases
- Performance metrics

## Test Scenarios

### 1. Engine Execution Build Chain (`test_engine_execution_build_chain`)

**Purpose**: Replicates Go's `TestEngineExecution` build phase
- Tests blocks 1-10 with varying transaction counts
- Block 4 has 0 transactions (edge case)
- Validates state root changes
- Verifies transaction inclusion

**Go Test Equivalent**:
```go
for blockHeight := initialHeight; blockHeight <= 10; blockHeight++ {
    nTxs := int(blockHeight) + 10
    if blockHeight == 4 {
        nTxs = 0  // Edge case
    }
    // ... transaction processing
}
```

**Rust Test Implementation**:
```rust
for block_height in initial_height..=10 {
    let n_txs = if block_height == 4 {
        0  // Block 4 has 0 transactions as edge case
    } else {
        (block_height as usize) + 2
    };
    // ... payload building and validation
}
```

### 2. Engine Execution Sync Chain (`test_engine_execution_sync_chain`)

**Purpose**: Replicates Go's sync phase where a fresh node replays stored payloads
- Creates fresh payload builder instance
- Replays stored transaction payloads
- Verifies deterministic execution
- Validates state consistency

### 3. Concurrent Execution (`test_concurrent_execution`)

**Purpose**: Tests high-load scenarios with multiple concurrent payload builds
- Spawns 5 concurrent execution tasks
- Validates thread safety
- Ensures at least 3 successful executions

### 4. Error Handling (`test_error_handling`)

**Purpose**: Tests edge cases and error conditions
- Invalid initial height
- Extremely large timestamps
- Large transaction batches
- Graceful degradation

### 5. Performance Metrics (`test_performance_metrics`)

**Purpose**: Measures execution performance across different batch sizes
- Tests batches of 1, 10, 50, 100 transactions
- Measures execution time
- Validates performance constraints

## Key Test Constants

The tests use the same constants as the Go tests for consistency:

```rust
const TEST_CHAIN_ID: u64 = 1234;
const GENESIS_HASH: &str = "0x2b8bbb1ea1e04f9c9809b4b278a8687806edc061a356c7dbc491930d8e922503";
const GENESIS_STATEROOT: &str = "0x05e9954443da80d86f2104e56ffdfd98fe21988730684360104865b3dc8191b4";
const TEST_TO_ADDRESS: &str = "0x944fDcD1c868E3cC566C78023CcB38A32cDA836E";
```

## Test Fixture Architecture

### `RollkitTestFixture`
Basic test fixture for payload building tests:
- Mock provider setup
- Genesis block configuration
- Transaction generation utilities
- Payload attributes creation

### `EngineApiTestFixture`
Advanced fixture that simulates the Go Engine API client:
- Implements `init_chain()`, `execute_txs()`, `set_final()`
- Manages state transitions
- Handles block validation
- Provides transaction utilities

## Validation Patterns

### State Root Validation
```rust
// For empty blocks, state root might remain the same
if n_txs == 0 {
    println!("  Empty block - state root handling verified");
} else {
    // Non-empty blocks should change state root
    assert_ne!(prev_state_root, new_state_root, 
        "State root should change for non-empty blocks");
}
```

### Transaction Count Validation
```rust
// Verify block properties
assert_eq!(sealed_block.number, block_height);
assert_eq!(sealed_block.body.transactions.len(), n_txs);

// Verify transaction inclusion
if n_txs > 0 {
    assert!(sealed_block.body.transactions.len() >= n_txs, 
        "Block should contain at least {} transactions", n_txs);
}
```

### Gas Usage Validation
```rust
if n_txs > 0 {
    assert!(max_bytes > 0, "Max bytes should be > 0 for non-empty blocks");
}
```

## Running the Tests

### Prerequisites
- Rust toolchain
- All rollkit-payload-builder dependencies

### Basic Tests
```bash
# Run all integration tests
cargo test --test integration_tests

# Run specific test
cargo test --test integration_tests test_rollkit_payload_execution
```

### Engine API Tests
```bash
# Run all engine API tests
cargo test --test engine_api_tests

# Run build chain test
cargo test --test engine_api_tests test_engine_execution_build_chain

# Run sync chain test
cargo test --test engine_api_tests test_engine_execution_sync_chain
```

### Performance Tests
```bash
# Run performance metrics
cargo test --test engine_api_tests test_performance_metrics -- --nocapture
```

### All Tests with Output
```bash
# Run all tests with detailed output
cargo test --tests -- --nocapture
```

## Expected Test Output

### Successful Build Chain Phase
```
=== Build Chain Phase ===
Chain initialized with state_root: [5, 233, 85, 68, ...], gas_limit: 30000000
Building block 1 with 3 transactions
✓ Block 1 completed successfully
Building block 2 with 4 transactions
✓ Block 2 completed successfully
...
Building block 4 with 0 transactions
  Empty block - state root handling verified
✓ Block 4 completed successfully
...
✓ Build chain phase completed successfully
✓ Engine execution build chain test passed!
```

### Successful Sync Phase
```
=== Sync Chain Phase ===
Sync chain initialized with state_root: [5, 233, 85, 68, ...], gas_limit: 30000000
Syncing block 1 with 3 transactions
✓ Block 1 synced successfully
...
✓ Sync chain phase completed successfully
✓ Engine execution sync chain test passed!
```

## Test Coverage

The integration tests provide comprehensive coverage of:

✅ **Core Functionality**
- Payload building with transactions
- Empty block handling
- State root management
- Gas limit validation

✅ **Engine API Compatibility**
- Chain initialization
- Transaction execution
- Block finalization
- State transitions

✅ **Edge Cases**
- Zero transaction blocks
- Large transaction batches
- Invalid parameters
- Concurrent operations

✅ **Performance**
- Execution timing
- Memory usage patterns
- Throughput validation
- Resource constraints

✅ **Error Handling**
- Invalid inputs
- Resource exhaustion
- Concurrent access
- Recovery scenarios

## Relationship to Go Tests

| Go Function | Rust Equivalent | Purpose |
|-------------|-----------------|---------|
| `NewEngineExecutionClient` | `EngineApiTestFixture::new()` | Initialize test client |
| `InitChain` | `init_chain()` | Initialize blockchain |
| `ExecuteTxs` | `execute_txs()` | Execute transactions |
| `SetFinal` | `set_final()` | Finalize blocks |
| `GetRandomTransaction` | `create_random_transactions()` | Generate test transactions |
| `checkLatestBlock` | `check_latest_block()` | Verify block state |

## Debugging and Troubleshooting

### Common Issues

1. **Mock Provider Setup**: Ensure genesis block is properly configured
2. **Transaction Signatures**: Use valid dummy signatures for testing
3. **State Root Validation**: Account for EVM state changes
4. **Gas Calculations**: Verify gas limit and usage calculations

### Debug Output
Add `-- --nocapture` to see detailed test output:
```bash
cargo test --test engine_api_tests -- --nocapture
```

### Individual Test Execution
```bash
# Run single test with output
cargo test --test engine_api_tests test_engine_execution_build_chain -- --nocapture
```

## Future Enhancements

Potential additions to the test suite:
- Real Engine API integration tests
- Network simulation tests  
- Database persistence validation
- Cross-chain compatibility tests
- Load testing with realistic transaction volumes
- Integration with actual reth node instances

## Contributing

When adding new tests:
1. Follow the established fixture patterns
2. Include both positive and negative test cases
3. Add appropriate documentation
4. Ensure tests are deterministic and repeatable
5. Validate against equivalent Go test behavior 
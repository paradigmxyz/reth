# Reth RPC E2E Tests

This crate contains end-to-end tests for Reth's RPC implementation, including compatibility testing against the official execution-apis test suite.

## Overview

The RPC compatibility testing framework enables:
1. Importing pre-built blockchain data from RLP files
2. Initializing nodes with specific forkchoice states
3. Running standardized RPC test cases from the execution-apis repository
4. Comparing responses against expected results

## Architecture

### Key Components

1. **`RunRpcCompatTests` Action**: Executes RPC test cases from .io files
2. **`InitializeFromExecutionApis` Action**: Applies forkchoice state from JSON files with automatic retry for syncing nodes
3. **Test Data Format**: Uses execution-apis .io file format for test cases

### Test Data Structure

Expected directory structure:
```
test_data_path/
├── chain.rlp           # Pre-built blockchain data
├── headfcu.json        # Initial forkchoice state
├── genesis.json        # Genesis configuration (optional)
└── eth_getLogs/        # Test cases for eth_getLogs
    ├── contract-addr.io
    ├── no-topics.io
    ├── topic-exact-match.io
    └── topic-wildcard.io
```

### .io File Format

Test files use a simple request-response format:
```
// Optional comment describing the test
// speconly: marks test as specification-only
>> {"jsonrpc":"2.0","id":1,"method":"eth_getLogs","params":[...]}
<< {"jsonrpc":"2.0","id":1,"result":[...]}
```

## Usage

### Basic Example

```rust
use alloy_genesis::Genesis;
use reth_chainspec::ChainSpec;
use reth_e2e_test_utils::testsuite::{
    actions::{MakeCanonical, UpdateBlockInfo},
    setup::{NetworkSetup, Setup},
    TestBuilder,
};
use reth_rpc_e2e_tests::rpc_compat::{InitializeFromExecutionApis, RunRpcCompatTests};

#[tokio::test]
async fn test_eth_get_logs_compat() -> Result<()> {
    let test_data_path = "../execution-apis/tests";
    let chain_rlp_path = PathBuf::from(&test_data_path).join("chain.rlp");
    let fcu_json_path = PathBuf::from(&test_data_path).join("headfcu.json");
    let genesis_path = PathBuf::from(&test_data_path).join("genesis.json");

    // Parse genesis.json to get chain spec with all hardfork configuration
    let genesis_json = std::fs::read_to_string(&genesis_path)?;
    let genesis: Genesis = serde_json::from_str(&genesis_json)?;
    let chain_spec: ChainSpec = genesis.into();
    let chain_spec = Arc::new(chain_spec);

    let setup = Setup::<EthEngineTypes>::default()
        .with_chain_spec(chain_spec)
        .with_network(NetworkSetup::single_node());

    let test = TestBuilder::new()
        .with_setup_and_import(setup, chain_rlp_path)
        .with_action(UpdateBlockInfo::default())
        .with_action(
            InitializeFromExecutionApis::new()
                .with_fcu_json(fcu_json_path.to_string_lossy()),
        )
        .with_action(MakeCanonical::new())
        .with_action(RunRpcCompatTests::new(
            vec!["eth_getLogs".to_string()],
            test_data_path.to_string_lossy(),
        ));

    test.run::<EthereumNode>().await?;
    Ok(())
}
```

### Running Tests

To run the official execution-apis test suite:

1. Clone the execution-apis repository:
   ```bash
   git clone https://github.com/ethereum/execution-apis.git
   ```

2. Set the test data path environment variable:
   ```bash
   export EXECUTION_APIS_TEST_PATH=/path/to/execution-apis/tests
   ```

3. Run the execution-apis compatibility test:
   ```bash
   cargo nextest run --test e2e_testsuite test_execution_apis_compat
   ```

This will auto-discover all RPC method directories and test each file individually, providing detailed per-file results.

### Custom Test Data

You can create custom test cases following the same format:

1. Create a directory structure matching the execution-apis format
2. Write .io files with request-response pairs
3. Use the same testing framework with your custom path

### Test Multiple RPC Methods

```rust
let methods_to_test = vec![
    "eth_blockNumber".to_string(),
    "eth_call".to_string(),
    "eth_getLogs".to_string(),
    "eth_getTransactionReceipt".to_string(),
];

RunRpcCompatTests::new(methods_to_test, test_data_path)
    .with_fail_fast(true)  // Stop on first failure
```

## Implementation Details

### JSON-RPC Request Handling

The framework handles various parameter formats:
- Empty parameters: `[]`
- Array parameters: `[param1, param2, ...]`
- Object parameters: Wrapped in array `[{...}]`

### Response Comparison

- **Numbers**: Compared with floating-point tolerance
- **Arrays**: Element-by-element comparison
- **Objects**: Key-by-key comparison (extra fields in actual response are allowed)
- **Errors**: Only presence is checked, not exact message

### Error Handling

- Parse errors are reported with context
- RPC errors are captured and compared
- Test failures include detailed diffs

## Benefits

1. **Standardization**: Uses official execution-apis test format
2. **Flexibility**: Works with custom test data
3. **Integration**: Seamlessly integrates with e2e test framework
4. **Extensibility**: Easy to add new RPC methods
5. **Debugging**: Detailed error reporting with fail-fast option

## Future Enhancements

- Support for batch requests
- WebSocket testing
- Performance benchmarking
- Automatic test discovery
- Parallel test execution
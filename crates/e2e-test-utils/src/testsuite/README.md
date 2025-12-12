# E2E Test Suite Framework

This directory contains the framework for writing end-to-end (e2e) tests in Reth. The framework provides utilities for setting up test environments, performing actions, and verifying blockchain behavior.

## Test Organization

E2E tests using this framework follow a consistent structure across the codebase:

### Directory Structure
Each crate that requires e2e tests should organize them as follows:
```
<crate-name>/
├── src/
│   └── ... (implementation code)
├── tests/
│   └── e2e-testsuite/
│       └── main.rs (or other test files)
└── Cargo.toml
```

### Cargo.toml Configuration
In your crate's `Cargo.toml`, define the e2e test binary:
```toml
[[test]]
name = "e2e_testsuite"
path = "tests/e2e-testsuite/main.rs"
harness = true
```

**Important**: The test binary MUST be named `e2e_testsuite` to be properly recognized by the nextest filter and CI workflows.

## Running E2E Tests

### Run all e2e tests across the workspace
```bash
cargo nextest run --workspace \
  --exclude 'example-*' \
  --exclude 'exex-subscription' \
  --exclude 'reth-bench' \
  --exclude 'ef-tests' \
  --exclude 'op-reth' \
  --exclude 'reth' \
  -E 'binary(e2e_testsuite)'
```

Note: The `--exclude` flags prevent compilation of crates that don't contain e2e tests (examples, benchmarks, binaries, and EF tests), significantly reducing build time.

### Run e2e tests for a specific crate
```bash
cargo nextest run -p <crate-name> -E 'binary(e2e_testsuite)'
```

### Run with additional features
```bash
cargo nextest run --locked --features "asm-keccak" --workspace -E 'binary(e2e_testsuite)'
```

### Run a specific test
```bash
cargo nextest run --workspace -E 'binary(e2e_testsuite) and test(test_name)'
```

## Writing E2E Tests

Tests use the framework components from this directory:

```rust
use reth_e2e_test_utils::{setup_import, Environment, TestBuilder};

#[tokio::test]
async fn test_example() -> eyre::Result<()> {
    // Create test environment
    let (mut env, mut handle) = TestBuilder::new()
        .build()
        .await?;

    // Perform test actions...
    
    Ok(())
}
```

## Framework Components

- **Environment**: Core test environment managing nodes and network state
- **TestBuilder**: Builder pattern for configuring test environments
- **Actions** (`actions/`): Pre-built test actions like block production, reorgs, etc.
- **Setup utilities**: Helper functions for common test scenarios

## CI Integration

E2E tests run in a dedicated GitHub Actions workflow (`.github/workflows/e2e.yml`) with:
- Extended timeouts (2 minutes per test, with 3 retries)
- Isolation from unit and integration tests
- Parallel execution support

## Nextest Configuration

The framework uses custom nextest settings (`.config/nextest.toml`):
```toml
[[profile.default.overrides]]
filter = "binary(e2e_testsuite)"
slow-timeout = { period = "2m", terminate-after = 3 }
```

This ensures all e2e tests get appropriate timeouts for complex blockchain operations.

## E2E Test Actions Reference

This section provides comprehensive documentation for all available end-to-end (e2e) test actions in the Reth testing framework. These actions enable developers to write complex blockchain integration tests by performing operations and making assertions in a single step.

### Overview

The e2e test framework provides a rich set of actions organized into several categories:

- **Block Production Actions**: Create and manage blocks
- **Fork Management Actions**: Handle blockchain forks and reorgs
- **Node Operations**: Multi-node coordination and validation
- **Engine API Actions**: Test execution layer interactions
- **RPC Compatibility Actions**: Test RPC methods against execution-apis test data
- **Custom FCU Actions**: Advanced forkchoice update scenarios

### Action Categories

#### Block Production Actions

##### `AssertMineBlock`
Mines a single block with specified transactions and verifies successful creation.

```rust
use reth_e2e_test_utils::testsuite::actions::AssertMineBlock;

let action = AssertMineBlock::new(
    node_idx,           // Node index to mine on
    transactions,       // Vec<Bytes> - transactions to include
    expected_hash,      // Option<B256> - expected block hash
    payload_attributes, // Engine::PayloadAttributes
);
```

##### `ProduceBlocks`
Produces a sequence of blocks using the available clients.

```rust
use reth_e2e_test_utils::testsuite::actions::ProduceBlocks;

let action = ProduceBlocks::new(num_blocks); // Number of blocks to produce
```

##### `ProduceBlocksLocally`
Produces blocks locally without broadcasting to other nodes.

```rust
use reth_e2e_test_utils::testsuite::actions::ProduceBlocksLocally;

let action = ProduceBlocksLocally::new(num_blocks);
```

##### `ProduceInvalidBlocks`
Produces a sequence of blocks where some blocks are intentionally invalid.

```rust
use reth_e2e_test_utils::testsuite::actions::ProduceInvalidBlocks;

let action = ProduceInvalidBlocks::new(
    num_blocks,        // Total number of blocks
    invalid_indices,   // HashSet<u64> - indices of invalid blocks
);

// Or create with a single invalid block
let action = ProduceInvalidBlocks::with_invalid_at(num_blocks, invalid_index);
```

##### `PickNextBlockProducer`
Selects the next block producer based on round-robin selection.

```rust
use reth_e2e_test_utils::testsuite::actions::PickNextBlockProducer;

let action = PickNextBlockProducer::new();
```

##### `GeneratePayloadAttributes`
Generates and stores payload attributes for the next block.

```rust
use reth_e2e_test_utils::testsuite::actions::GeneratePayloadAttributes;

let action = GeneratePayloadAttributes::new();
```

##### `GenerateNextPayload`
Generates the next execution payload using stored attributes.

```rust
use reth_e2e_test_utils::testsuite::actions::GenerateNextPayload;

let action = GenerateNextPayload::new();
```

##### `BroadcastLatestForkchoice`
Broadcasts the latest fork choice state to all clients.

```rust
use reth_e2e_test_utils::testsuite::actions::BroadcastLatestForkchoice;

let action = BroadcastLatestForkchoice::new();
```

##### `BroadcastNextNewPayload`
Broadcasts the next new payload to nodes.

```rust
use reth_e2e_test_utils::testsuite::actions::BroadcastNextNewPayload;

// Broadcast to all nodes
let action = BroadcastNextNewPayload::new();

// Broadcast only to active node
let action = BroadcastNextNewPayload::with_active_node();
```

##### `CheckPayloadAccepted`
Verifies that a broadcasted payload has been accepted by nodes.

```rust
use reth_e2e_test_utils::testsuite::actions::CheckPayloadAccepted;

let action = CheckPayloadAccepted::new();
```

##### `UpdateBlockInfo`
Syncs environment state with the node's canonical chain via RPC.

```rust
use reth_e2e_test_utils::testsuite::actions::UpdateBlockInfo;

let action = UpdateBlockInfo::new();
```

##### `UpdateBlockInfoToLatestPayload`
Updates environment state using the locally produced payload.

```rust
use reth_e2e_test_utils::testsuite::actions::UpdateBlockInfoToLatestPayload;

let action = UpdateBlockInfoToLatestPayload::new();
```

##### `MakeCanonical`
Makes the current latest block canonical by broadcasting a forkchoice update.

```rust
use reth_e2e_test_utils::testsuite::actions::MakeCanonical;

// Broadcast to all nodes
let action = MakeCanonical::new();

// Only apply to active node
let action = MakeCanonical::with_active_node();
```

##### `CaptureBlock`
Captures the current block and tags it with a name for later reference.

```rust
use reth_e2e_test_utils::testsuite::actions::CaptureBlock;

let action = CaptureBlock::new("block_tag");
```

#### Fork Management Actions

##### `CreateFork`
Creates a fork from a specified block and produces blocks on top.

```rust
use reth_e2e_test_utils::testsuite::actions::CreateFork;

// Create fork from block number
let action = CreateFork::new(fork_base_block, num_blocks);

// Create fork from tagged block
let action = CreateFork::new_from_tag("block_tag", num_blocks);
```

##### `SetForkBase`
Sets the fork base block in the environment.

```rust
use reth_e2e_test_utils::testsuite::actions::SetForkBase;

let action = SetForkBase::new(fork_base_block);
```

##### `SetForkBaseFromBlockInfo`
Sets the fork base from existing block information.

```rust
use reth_e2e_test_utils::testsuite::actions::SetForkBaseFromBlockInfo;

let action = SetForkBaseFromBlockInfo::new(block_info);
```

##### `ValidateFork`
Validates that a fork was created correctly.

```rust
use reth_e2e_test_utils::testsuite::actions::ValidateFork;

let action = ValidateFork::new(fork_base_number);
```

#### Reorg Actions

##### `ReorgTo`
Performs a reorg by setting a new head block as canonical.

```rust
use reth_e2e_test_utils::testsuite::actions::ReorgTo;

// Reorg to specific block hash
let action = ReorgTo::new(target_hash);

// Reorg to tagged block
let action = ReorgTo::new_from_tag("block_tag");
```

##### `SetReorgTarget`
Sets the reorg target block in the environment.

```rust
use reth_e2e_test_utils::testsuite::actions::SetReorgTarget;

let action = SetReorgTarget::new(target_block_info);
```

#### Node Operations

##### `SelectActiveNode`
Selects which node should be active for subsequent operations.

```rust
use reth_e2e_test_utils::testsuite::actions::SelectActiveNode;

let action = SelectActiveNode::new(node_idx);
```

##### `CompareNodeChainTips`
Compares chain tips between two nodes.

```rust
use reth_e2e_test_utils::testsuite::actions::CompareNodeChainTips;

// Expect nodes to have the same chain tip
let action = CompareNodeChainTips::expect_same(node_a, node_b);

// Expect nodes to have different chain tips
let action = CompareNodeChainTips::expect_different(node_a, node_b);
```

##### `CaptureBlockOnNode`
Captures a block with a tag, associating it with a specific node.

```rust
use reth_e2e_test_utils::testsuite::actions::CaptureBlockOnNode;

let action = CaptureBlockOnNode::new("tag_name", node_idx);
```

##### `ValidateBlockTag`
Validates that a block tag exists and optionally came from a specific node.

```rust
use reth_e2e_test_utils::testsuite::actions::ValidateBlockTag;

// Just validate tag exists
let action = ValidateBlockTag::exists("tag_name");

// Validate tag came from specific node
let action = ValidateBlockTag::from_node("tag_name", node_idx);
```

##### `WaitForSync`
Waits for two nodes to sync and have the same chain tip.

```rust
use reth_e2e_test_utils::testsuite::actions::WaitForSync;

// With default timeouts (30s timeout, 1s poll interval)
let action = WaitForSync::new(node_a, node_b);

// With custom timeouts
let action = WaitForSync::new(node_a, node_b)
    .with_timeout(60)        // 60 second timeout
    .with_poll_interval(2);  // 2 second poll interval
```

##### `AssertChainTip`
Asserts that the current chain tip is at a specific block number.

```rust
use reth_e2e_test_utils::testsuite::actions::AssertChainTip;

let action = AssertChainTip::new(expected_block_number);
```

#### Engine API Actions

##### `SendNewPayload`
Sends a newPayload request to a specific node.

```rust
use reth_e2e_test_utils::testsuite::actions::{SendNewPayload, ExpectedPayloadStatus};

let action = SendNewPayload::new(
    node_idx,                    // Target node index
    block_number,               // Block number to send
    source_node_idx,            // Source node to get block from
    ExpectedPayloadStatus::Valid, // Expected status
);
```

##### `SendNewPayloads`
Sends multiple blocks to a node in a specific order.

```rust
use reth_e2e_test_utils::testsuite::actions::SendNewPayloads;

let action = SendNewPayloads::new()
    .with_target_node(node_idx)
    .with_source_node(source_idx)
    .with_start_block(1)
    .with_total_blocks(5);

// Send in reverse order
let action = SendNewPayloads::new()
    .with_target_node(node_idx)
    .with_source_node(source_idx)
    .with_start_block(1)
    .with_total_blocks(5)
    .in_reverse_order();

// Send specific block numbers
let action = SendNewPayloads::new()
    .with_target_node(node_idx)
    .with_source_node(source_idx)
    .with_block_numbers(vec![1, 3, 5]);
```

#### RPC Compatibility Actions

##### `RunRpcCompatTests`
Runs RPC compatibility tests from execution-apis test data.

```rust
use reth_rpc_e2e_tests::rpc_compat::RunRpcCompatTests;

// Test specific RPC methods
let action = RunRpcCompatTests::new(
    vec!["eth_getLogs".to_string(), "eth_syncing".to_string()],
    test_data_path,
);

// With fail-fast option
let action = RunRpcCompatTests::new(methods, test_data_path)
    .with_fail_fast(true);
```

##### `InitializeFromExecutionApis`
Initializes the chain from execution-apis test data.

```rust
use reth_rpc_e2e_tests::rpc_compat::InitializeFromExecutionApis;

// With default paths
let action = InitializeFromExecutionApis::new();

// With custom paths
let action = InitializeFromExecutionApis::new()
    .with_chain_rlp("path/to/chain.rlp")
    .with_fcu_json("path/to/headfcu.json");
```

#### Custom FCU Actions

##### `SendForkchoiceUpdate`
Sends a custom forkchoice update with specific finalized, safe, and head blocks.

```rust
use reth_e2e_test_utils::testsuite::actions::{SendForkchoiceUpdate, BlockReference};

let action = SendForkchoiceUpdate::new(
    BlockReference::Hash(finalized_hash),
    BlockReference::Hash(safe_hash),
    BlockReference::Hash(head_hash),
);

// With expected status
let action = SendForkchoiceUpdate::new(
    BlockReference::Tag("finalized"),
    BlockReference::Tag("safe"),
    BlockReference::Tag("head"),
).with_expected_status(PayloadStatusEnum::Valid);

// Send to specific node
let action = SendForkchoiceUpdate::new(
    BlockReference::Latest,
    BlockReference::Latest,
    BlockReference::Latest,
).with_node_idx(node_idx);
```

##### `FinalizeBlock`
Finalizes a specific block with a given head.

```rust
use reth_e2e_test_utils::testsuite::actions::FinalizeBlock;

let action = FinalizeBlock::new(BlockReference::Hash(block_hash));

// With different head
let action = FinalizeBlock::new(BlockReference::Hash(block_hash))
    .with_head(BlockReference::Hash(head_hash));

// Send to specific node
let action = FinalizeBlock::new(BlockReference::Tag("block_tag"))
    .with_node_idx(node_idx);
```

#### FCU Status Testing Actions

##### `TestFcuToTag`
Tests forkchoice update to a tagged block with expected status.

```rust
use reth_e2e_test_utils::testsuite::actions::TestFcuToTag;

let action = TestFcuToTag::new("block_tag", PayloadStatusEnum::Valid);
```

##### `ExpectFcuStatus`
Expects a specific FCU status when targeting a tagged block.

```rust
use reth_e2e_test_utils::testsuite::actions::ExpectFcuStatus;

// Expect valid status
let action = ExpectFcuStatus::valid("block_tag");

// Expect invalid status
let action = ExpectFcuStatus::invalid("block_tag");

// Expect syncing status
let action = ExpectFcuStatus::syncing("block_tag");

// Expect accepted status
let action = ExpectFcuStatus::accepted("block_tag");
```

##### `ValidateCanonicalTag`
Validates that a tagged block remains canonical.

```rust
use reth_e2e_test_utils::testsuite::actions::ValidateCanonicalTag;

let action = ValidateCanonicalTag::new("block_tag");
```

### Block Reference Types

#### `BlockReference`
Used to reference blocks in various actions:

```rust
use reth_e2e_test_utils::testsuite::actions::BlockReference;

// Direct block hash
let reference = BlockReference::Hash(block_hash);

// Tagged block reference
let reference = BlockReference::Tag("block_tag".to_string());

// Latest block on active node
let reference = BlockReference::Latest;
```

#### `ForkBase`
Used to specify fork base in fork creation:

```rust
use reth_e2e_test_utils::testsuite::actions::ForkBase;

// Block number
let fork_base = ForkBase::Number(block_number);

// Tagged block
let fork_base = ForkBase::Tag("block_tag".to_string());
```

#### `ReorgTarget`
Used to specify reorg targets:

```rust
use reth_e2e_test_utils::testsuite::actions::ReorgTarget;

// Direct block hash
let target = ReorgTarget::Hash(block_hash);

// Tagged block reference
let target = ReorgTarget::Tag("block_tag".to_string());
```

### Expected Payload Status

#### `ExpectedPayloadStatus`
Used to specify expected payload status in engine API actions:

```rust
use reth_e2e_test_utils::testsuite::actions::ExpectedPayloadStatus;

// Expect valid payload
let status = ExpectedPayloadStatus::Valid;

// Expect invalid payload
let status = ExpectedPayloadStatus::Invalid;

// Expect syncing or accepted (buffered)
let status = ExpectedPayloadStatus::SyncingOrAccepted;
```

### Usage Examples

#### Basic Block Production Test

```rust
use reth_e2e_test_utils::testsuite::{
    actions::{ProduceBlocks, MakeCanonical, AssertChainTip},
    setup::{NetworkSetup, Setup},
    TestBuilder,
};

#[tokio::test]
async fn test_basic_block_production() -> eyre::Result<()> {
    let setup = Setup::default()
        .with_chain_spec(chain_spec)
        .with_network(NetworkSetup::single_node());

    let test = TestBuilder::new()
        .with_setup(setup)
        .with_action(ProduceBlocks::new(5))
        .with_action(MakeCanonical::new())
        .with_action(AssertChainTip::new(5));

    test.run::<EthereumNode>().await?;
    Ok(())
}
```

#### Fork and Reorg Test

```rust
use reth_e2e_test_utils::testsuite::{
    actions::{ProduceBlocks, CreateFork, CaptureBlock, ReorgTo, MakeCanonical},
    setup::{NetworkSetup, Setup},
    TestBuilder,
};

#[tokio::test]
async fn test_fork_and_reorg() -> eyre::Result<()> {
    let setup = Setup::default()
        .with_chain_spec(chain_spec)
        .with_network(NetworkSetup::single_node());

    let test = TestBuilder::new()
        .with_setup(setup)
        .with_action(ProduceBlocks::new(3))           // Produce blocks 1, 2, 3
        .with_action(MakeCanonical::new())            // Make main chain canonical
        .with_action(CreateFork::new(1, 2))          // Fork from block 1, produce 2 blocks
        .with_action(CaptureBlock::new("fork_tip"))  // Tag the fork tip
        .with_action(ReorgTo::new_from_tag("fork_tip")); // Reorg to fork tip

    test.run::<EthereumNode>().await?;
    Ok(())
}
```

#### Multi-Node Test

```rust
use reth_e2e_test_utils::testsuite::{
    actions::{SelectActiveNode, ProduceBlocks, CompareNodeChainTips, CaptureBlockOnNode},
    setup::{NetworkSetup, Setup},
    TestBuilder,
};

#[tokio::test]
async fn test_multi_node_coordination() -> eyre::Result<()> {
    let setup = Setup::default()
        .with_chain_spec(chain_spec)
        .with_network(NetworkSetup::multi_node(2)); // 2 nodes

    let test = TestBuilder::new()
        .with_setup(setup)
        .with_action(CompareNodeChainTips::expect_same(0, 1))  // Both start at genesis
        .with_action(SelectActiveNode::new(0))                 // Select node 0
        .with_action(ProduceBlocks::new(3))                    // Produce blocks on node 0
        .with_action(CaptureBlockOnNode::new("node0_tip", 0))  // Tag node 0's tip
        .with_action(CompareNodeChainTips::expect_same(0, 1)); // Verify sync

    test.run::<EthereumNode>().await?;
    Ok(())
}
```

#### Engine API Test

```rust
use reth_e2e_test_utils::testsuite::{
    actions::{SendNewPayload, ExpectedPayloadStatus},
    setup::{NetworkSetup, Setup},
    TestBuilder,
};

#[tokio::test]
async fn test_engine_api() -> eyre::Result<()> {
    let setup = Setup::default()
        .with_chain_spec(chain_spec)
        .with_network(NetworkSetup::multi_node(2));

    let test = TestBuilder::new()
        .with_setup(setup)
        .with_action(SendNewPayload::new(
            1,                                    // Target node
            1,                                    // Block number
            0,                                    // Source node
            ExpectedPayloadStatus::Valid,         // Expected status
        ));

    test.run::<EthereumNode>().await?;
    Ok(())
}
```

#### RPC Compatibility Test

```rust
use reth_e2e_test_utils::testsuite::{
    actions::{MakeCanonical, UpdateBlockInfo},
    setup::{NetworkSetup, Setup},
    TestBuilder,
};
use reth_rpc_e2e_tests::rpc_compat::{InitializeFromExecutionApis, RunRpcCompatTests};

#[tokio::test]
async fn test_rpc_compatibility() -> eyre::Result<()> {
    let test_data_path = "path/to/execution-apis/tests";
    
    let setup = Setup::default()
        .with_chain_spec(chain_spec)
        .with_network(NetworkSetup::single_node());

    let test = TestBuilder::new()
        .with_setup_and_import(setup, "path/to/chain.rlp")
        .with_action(UpdateBlockInfo::default())
        .with_action(InitializeFromExecutionApis::new()
            .with_fcu_json("path/to/headfcu.json"))
        .with_action(MakeCanonical::new())
        .with_action(RunRpcCompatTests::new(
            vec!["eth_getLogs".to_string()],
            test_data_path,
        ));

    test.run::<EthereumNode>().await?;
    Ok(())
}
```

### Best Practices

1. **Use Tagged Blocks**: Use `CaptureBlock` or `CaptureBlockOnNode` to tag important blocks for later reference in reorgs and forks.

2. **Make Blocks Canonical**: After producing blocks, use `MakeCanonical` to ensure they become part of the canonical chain.

3. **Update Block Info**: Use `UpdateBlockInfo` or `UpdateBlockInfoToLatestPayload` to keep the environment state synchronized with the node.

4. **Multi-Node Coordination**: Use `SelectActiveNode` to control which node performs operations, and `CompareNodeChainTips` to verify synchronization.

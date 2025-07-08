//! RPC compatibility tests using execution-apis test data

use alloy_genesis::Genesis;
use eyre::Result;
use reth_chainspec::ChainSpec;
use reth_e2e_test_utils::testsuite::{
    actions::{MakeCanonical, UpdateBlockInfo},
    setup::{NetworkSetup, Setup},
    TestBuilder,
};
use reth_node_ethereum::{EthEngineTypes, EthereumNode};
use reth_rpc_e2e_tests::rpc_compat::{InitializeFromExecutionApis, RunRpcCompatTests};
use std::{path::PathBuf, sync::Arc};
use tracing::info;

/// Test `eth_getLogs` RPC method compatibility with execution-apis test data
///
/// This test:
/// 1. Initializes a node with chain data from testdata (chain.rlp)
/// 2. Applies the forkchoice state from headfcu.json
/// 3. Runs all `eth_getLogs` test cases from the execution-apis test suite
#[tokio::test(flavor = "multi_thread")]
async fn test_eth_get_logs_compat() -> Result<()> {
    reth_tracing::init_test_tracing();

    // Use local test data
    let test_data_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/rpc-compat");

    assert!(test_data_path.exists(), "Test data path does not exist: {}", test_data_path.display());

    info!("Using test data from: {}", test_data_path.display());

    // Paths to test files
    let chain_rlp_path = test_data_path.join("chain.rlp");
    let fcu_json_path = test_data_path.join("headfcu.json");
    let genesis_path = test_data_path.join("genesis.json");

    // Verify required files exist
    if !chain_rlp_path.exists() {
        return Err(eyre::eyre!("chain.rlp not found at {}", chain_rlp_path.display()));
    }
    if !fcu_json_path.exists() {
        return Err(eyre::eyre!("headfcu.json not found at {}", fcu_json_path.display()));
    }
    if !genesis_path.exists() {
        return Err(eyre::eyre!("genesis.json not found at {}", genesis_path.display()));
    }

    // Load genesis from test data
    let genesis_json = std::fs::read_to_string(&genesis_path)?;

    // Parse the Genesis struct from JSON and convert it to ChainSpec
    // This properly handles all the hardfork configuration from the config section
    let genesis: Genesis = serde_json::from_str(&genesis_json)?;
    let chain_spec: ChainSpec = genesis.into();
    let chain_spec = Arc::new(chain_spec);

    // Create test setup with imported chain
    let setup = Setup::<EthEngineTypes>::default()
        .with_chain_spec(chain_spec)
        .with_network(NetworkSetup::single_node());

    // Build and run the test
    let test = TestBuilder::new()
        .with_setup_and_import(setup, chain_rlp_path)
        .with_action(UpdateBlockInfo::default())
        .with_action(
            InitializeFromExecutionApis::new().with_fcu_json(fcu_json_path.to_string_lossy()),
        )
        .with_action(MakeCanonical::new())
        .with_action(RunRpcCompatTests::new(
            vec!["eth_getLogs".to_string()],
            test_data_path.to_string_lossy(),
        ));

    test.run::<EthereumNode>().await?;

    Ok(())
}

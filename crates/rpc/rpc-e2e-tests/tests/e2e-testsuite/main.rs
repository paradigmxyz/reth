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
use std::{env, path::PathBuf, sync::Arc};
use tracing::{debug, info};

/// Test repo-local RPC method compatibility with execution-apis test data
///
/// This test:
/// 1. Initializes a node with chain data from testdata (chain.rlp)
/// 2. Applies the forkchoice state from headfcu.json
/// 3. Runs tests cases in the local repository, some of which are execution-api tests
#[tokio::test(flavor = "multi_thread")]
async fn test_local_rpc_tests_compat() -> Result<()> {
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
            vec!["eth_getLogs".to_string(), "eth_syncing".to_string()],
            test_data_path.to_string_lossy(),
        ));

    test.run::<EthereumNode>().await?;

    Ok(())
}

/// Test RPC method compatibility with execution-apis test data from environment variable
///
/// This test:
/// 1. Reads test data path from `EXECUTION_APIS_TEST_PATH` environment variable
/// 2. Auto-discovers all RPC method directories (starting with `eth_`)
/// 3. Initializes a node with chain data from that directory (chain.rlp)
/// 4. Applies the forkchoice state from headfcu.json
/// 5. Runs all discovered RPC test cases individually (each test file reported separately)
#[tokio::test(flavor = "multi_thread")]
async fn test_execution_apis_compat() -> Result<()> {
    reth_tracing::init_test_tracing();

    // Get test data path from environment variable
    let test_data_path = match env::var("EXECUTION_APIS_TEST_PATH") {
        Ok(path) => path,
        Err(_) => {
            info!("SKIPPING: EXECUTION_APIS_TEST_PATH environment variable not set. Please set it to the path of execution-apis/tests directory to run this test.");
            return Ok(());
        }
    };

    let test_data_path = PathBuf::from(test_data_path);

    if !test_data_path.exists() {
        return Err(eyre::eyre!("Test data path does not exist: {}", test_data_path.display()));
    }

    info!("Using execution-apis test data from: {}", test_data_path.display());

    // Auto-discover RPC method directories
    let mut rpc_methods = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&test_data_path) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                // Search for an underscore to get all namespaced directories
                if entry.path().is_dir() && name.contains('_') {
                    rpc_methods.push(name.to_string());
                }
            }
        }
    }

    if rpc_methods.is_empty() {
        return Err(eyre::eyre!(
            "No RPC method directories (containing a '_' indicating namespacing) found in {}",
            test_data_path.display()
        ));
    }

    rpc_methods.sort();
    debug!("Found RPC method test directories: {:?}", rpc_methods);

    // Paths to chain config files
    let chain_rlp_path = test_data_path.join("chain.rlp");
    let genesis_path = test_data_path.join("genesis.json");
    let fcu_json_path = test_data_path.join("headfcu.json");

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
    let genesis: Genesis = serde_json::from_str(&genesis_json)?;
    let chain_spec: ChainSpec = genesis.into();
    let chain_spec = Arc::new(chain_spec);

    // Create test setup with imported chain
    let setup = Setup::<EthEngineTypes>::default()
        .with_chain_spec(chain_spec)
        .with_network(NetworkSetup::single_node());

    // Build and run the test with all discovered methods
    let test = TestBuilder::new()
        .with_setup_and_import(setup, chain_rlp_path)
        .with_action(UpdateBlockInfo::default())
        .with_action(
            InitializeFromExecutionApis::new().with_fcu_json(fcu_json_path.to_string_lossy()),
        )
        .with_action(MakeCanonical::new())
        .with_action(RunRpcCompatTests::new(rpc_methods, test_data_path.to_string_lossy()));

    test.run::<EthereumNode>().await?;

    Ok(())
}

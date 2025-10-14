//! Setup utilities for importing RLP chain data before starting nodes.

use crate::{node::NodeTestContext, NodeHelperType, Wallet};
use reth_chainspec::ChainSpec;
use reth_cli_commands::import_core::{import_blocks_from_file, ImportConfig};
use reth_config::Config;
use reth_db::DatabaseEnv;
use reth_node_api::{NodeTypesWithDBAdapter, TreeConfig};
use reth_node_builder::{EngineNodeLauncher, Node, NodeBuilder, NodeConfig, NodeHandle};
use reth_node_core::args::{DiscoveryArgs, NetworkArgs, RpcServerArgs};
use reth_node_ethereum::EthereumNode;
use reth_provider::{
    providers::BlockchainProvider, DatabaseProviderFactory, ProviderFactory, StageCheckpointReader,
    StaticFileProviderFactory,
};
use reth_rpc_server_types::RpcModuleSelection;
use reth_stages_types::StageId;
use reth_tasks::TaskManager;
use std::{path::Path, sync::Arc};
use tempfile::TempDir;
use tracing::{debug, info, span, Level};

/// Setup result containing nodes and temporary directories that must be kept alive
pub struct ChainImportResult {
    /// The nodes that were created
    pub nodes: Vec<NodeHelperType<EthereumNode>>,
    /// The task manager
    pub task_manager: TaskManager,
    /// The wallet for testing
    pub wallet: Wallet,
    /// Temporary directories that must be kept alive for the duration of the test
    pub _temp_dirs: Vec<TempDir>,
}

impl std::fmt::Debug for ChainImportResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChainImportResult")
            .field("nodes", &self.nodes.len())
            .field("wallet", &self.wallet)
            .field("temp_dirs", &self._temp_dirs.len())
            .finish()
    }
}

/// Creates a test setup with Ethereum nodes that have pre-imported chain data from RLP files.
///
/// This function:
/// 1. Creates a temporary datadir for each node
/// 2. Imports the specified RLP chain data into the datadir
/// 3. Starts the nodes with the pre-populated database
/// 4. Returns the running nodes ready for testing
///
/// Note: This function is currently specific to `EthereumNode` because the import process
/// uses Ethereum-specific consensus and block format. It can be made generic in the future
/// by abstracting the import process.
/// It uses `NoopConsensus` during import to bypass validation checks like gas limit constraints,
/// which allows importing test chains that may not strictly conform to mainnet consensus rules. The
/// nodes themselves still run with proper consensus when started.
pub async fn setup_engine_with_chain_import(
    num_nodes: usize,
    chain_spec: Arc<ChainSpec>,
    is_dev: bool,
    tree_config: TreeConfig,
    rlp_path: &Path,
    attributes_generator: impl Fn(u64) -> reth_payload_builder::EthPayloadBuilderAttributes
        + Send
        + Sync
        + Copy
        + 'static,
) -> eyre::Result<ChainImportResult> {
    let tasks = TaskManager::current();
    let exec = tasks.executor();

    let network_config = NetworkArgs {
        discovery: DiscoveryArgs { disable_discovery: true, ..DiscoveryArgs::default() },
        ..NetworkArgs::default()
    };

    // Create nodes with imported data
    let mut nodes: Vec<NodeHelperType<EthereumNode>> = Vec::with_capacity(num_nodes);
    let mut temp_dirs = Vec::with_capacity(num_nodes); // Keep temp dirs alive

    for idx in 0..num_nodes {
        // Create a temporary datadir for this node
        let temp_dir = TempDir::new()?;
        let datadir = temp_dir.path().to_path_buf();

        let mut node_config = NodeConfig::new(chain_spec.clone())
            .with_network(network_config.clone())
            .with_unused_ports()
            .with_rpc(
                RpcServerArgs::default()
                    .with_unused_ports()
                    .with_http()
                    .with_http_api(RpcModuleSelection::All),
            )
            .set_dev(is_dev);

        // Set the datadir
        node_config.datadir.datadir =
            reth_node_core::dirs::MaybePlatformPath::from(datadir.clone());
        debug!(target: "e2e::import", "Node {idx} datadir: {datadir:?}");

        let span = span!(Level::INFO, "node", idx);
        let _enter = span.enter();

        // First, import the chain data into this datadir
        info!(target: "test", "Importing chain data from {:?} for node {} into {:?}", rlp_path, idx, datadir);

        // Create database path and static files path
        let db_path = datadir.join("db");
        let static_files_path = datadir.join("static_files");

        // Initialize the database using init_db (same as CLI import command)
        // Use the same database arguments as the node will use
        let db_args = reth_node_core::args::DatabaseArgs::default().database_args();
        let db_env = reth_db::init_db(&db_path, db_args)?;
        let db = Arc::new(db_env);

        // Create a provider factory with the initialized database (use regular DB, not
        // TempDatabase) We need to specify the node types properly for the adapter
        let provider_factory = ProviderFactory::<
            NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>,
        >::new(
            db.clone(),
            chain_spec.clone(),
            reth_provider::providers::StaticFileProvider::read_write(static_files_path.clone())?,
        );

        // Initialize genesis if needed
        reth_db_common::init::init_genesis(&provider_factory)?;

        // Import the chain data
        // Use no_state to skip state validation for test chains
        let import_config = ImportConfig::default();
        let config = Config::default();

        // Create EVM and consensus for Ethereum
        let evm_config = reth_node_ethereum::EthEvmConfig::new(chain_spec.clone());
        // Use NoopConsensus to skip gas limit validation for test imports
        let consensus = reth_consensus::noop::NoopConsensus::arc();

        let result = import_blocks_from_file(
            rlp_path,
            import_config,
            provider_factory.clone(),
            &config,
            evm_config,
            consensus,
        )
        .await?;

        info!(
            target: "test",
            "Imported {} blocks and {} transactions for node {}",
            result.total_imported_blocks,
            result.total_imported_txns,
            idx
        );

        debug!(target: "e2e::import",
            "Import result for node {}: decoded {} blocks, imported {} blocks, complete: {}",
            idx,
            result.total_decoded_blocks,
            result.total_imported_blocks,
            result.is_complete()
        );

        if result.total_decoded_blocks != result.total_imported_blocks {
            debug!(target: "e2e::import",
                "Import block count mismatch: decoded {} != imported {}",
                result.total_decoded_blocks, result.total_imported_blocks
            );
            return Err(eyre::eyre!("Chain import block count mismatch for node {}", idx));
        }

        if result.total_decoded_txns != result.total_imported_txns {
            debug!(target: "e2e::import",
                "Import transaction count mismatch: decoded {} != imported {}",
                result.total_decoded_txns, result.total_imported_txns
            );
            return Err(eyre::eyre!("Chain import transaction count mismatch for node {}", idx));
        }

        // Verify the database was properly initialized by checking stage checkpoints
        {
            let provider = provider_factory.database_provider_ro()?;
            let headers_checkpoint = provider.get_stage_checkpoint(StageId::Headers)?;
            if headers_checkpoint.is_none() {
                return Err(eyre::eyre!("Headers stage checkpoint is missing after import!"));
            }
            debug!(target: "e2e::import", "Headers stage checkpoint after import: {headers_checkpoint:?}");
            drop(provider);
        }

        // IMPORTANT: We need to properly flush and close the static files provider
        // The static files provider may have open file handles that need to be closed
        // before we can reopen the database in the node launcher
        {
            let static_file_provider = provider_factory.static_file_provider();
            // This will ensure all static file writers are properly closed
            drop(static_file_provider);
        }

        // Close all database handles to release locks before launching the node
        drop(provider_factory);
        drop(db);

        // Give the OS a moment to release file locks
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Now launch the node with the pre-populated datadir
        debug!(target: "e2e::import", "Launching node with datadir: {:?}", datadir);

        // Use the testing_node_with_datadir method which properly handles opening existing
        // databases
        let node = EthereumNode::default();

        let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config.clone())
            .testing_node_with_datadir(exec.clone(), datadir.clone())
            .with_types_and_provider::<EthereumNode, BlockchainProvider<_>>()
            .with_components(node.components_builder())
            .with_add_ons(node.add_ons())
            .launch_with_fn(|builder| {
                let launcher = EngineNodeLauncher::new(
                    builder.task_executor().clone(),
                    builder.config().datadir(),
                    tree_config.clone(),
                );
                builder.launch_with(launcher)
            })
            .await?;

        let node_ctx = NodeTestContext::new(node, attributes_generator).await?;

        nodes.push(node_ctx);
        temp_dirs.push(temp_dir); // Keep temp dir alive
    }

    Ok(ChainImportResult {
        nodes,
        task_manager: tasks,
        wallet: crate::Wallet::default().with_chain_id(chain_spec.chain.id()),
        _temp_dirs: temp_dirs,
    })
}

/// Helper to load forkchoice state from a JSON file
pub fn load_forkchoice_state(path: &Path) -> eyre::Result<alloy_rpc_types_engine::ForkchoiceState> {
    let json_str = std::fs::read_to_string(path)?;
    let fcu_data: serde_json::Value = serde_json::from_str(&json_str)?;

    // The headfcu.json file contains a JSON-RPC request with the forkchoice state in params[0]
    let state = &fcu_data["params"][0];
    Ok(alloy_rpc_types_engine::ForkchoiceState {
        head_block_hash: state["headBlockHash"]
            .as_str()
            .ok_or_else(|| eyre::eyre!("missing headBlockHash"))?
            .parse()?,
        safe_block_hash: state["safeBlockHash"]
            .as_str()
            .ok_or_else(|| eyre::eyre!("missing safeBlockHash"))?
            .parse()?,
        finalized_block_hash: state["finalizedBlockHash"]
            .as_str()
            .ok_or_else(|| eyre::eyre!("missing finalizedBlockHash"))?
            .parse()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_rlp_utils::{create_fcu_json, generate_test_blocks, write_blocks_to_rlp};
    use reth_chainspec::{ChainSpecBuilder, MAINNET};
    use reth_db::DatabaseArguments;
    use reth_payload_builder::EthPayloadBuilderAttributes;
    use reth_primitives::SealedBlock;
    use reth_provider::{
        test_utils::MockNodeTypesWithDB, BlockHashReader, BlockNumReader, BlockReaderIdExt,
    };
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_stage_checkpoints_persistence() {
        // This test specifically verifies that stage checkpoints are persisted correctly
        // when reopening the database
        reth_tracing::init_test_tracing();

        let chain_spec = Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(
                    serde_json::from_str(include_str!("testsuite/assets/genesis.json")).unwrap(),
                )
                .london_activated()
                .shanghai_activated()
                .build(),
        );

        // Generate test blocks
        let test_blocks = generate_test_blocks(&chain_spec, 5);

        // Create temporary files for RLP data
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let rlp_path = temp_dir.path().join("test_chain.rlp");
        write_blocks_to_rlp(&test_blocks, &rlp_path).expect("Failed to write RLP data");

        // Create a persistent datadir that won't be deleted
        let datadir = temp_dir.path().join("datadir");
        std::fs::create_dir_all(&datadir).unwrap();
        let db_path = datadir.join("db");
        let static_files_path = datadir.join("static_files");

        // Import the chain
        {
            let db_env = reth_db::init_db(&db_path, DatabaseArguments::default()).unwrap();
            let db = Arc::new(db_env);

            let provider_factory: ProviderFactory<
                NodeTypesWithDBAdapter<reth_node_ethereum::EthereumNode, Arc<DatabaseEnv>>,
            > = ProviderFactory::new(
                db.clone(),
                chain_spec.clone(),
                reth_provider::providers::StaticFileProvider::read_write(static_files_path.clone())
                    .unwrap(),
            );

            // Initialize genesis
            reth_db_common::init::init_genesis(&provider_factory).unwrap();

            // Import the chain data
            let import_config = ImportConfig::default();
            let config = Config::default();
            let evm_config = reth_node_ethereum::EthEvmConfig::new(chain_spec.clone());
            // Use NoopConsensus to skip gas limit validation for test imports
            let consensus = reth_consensus::noop::NoopConsensus::arc();

            let result = import_blocks_from_file(
                &rlp_path,
                import_config,
                provider_factory.clone(),
                &config,
                evm_config,
                consensus,
            )
            .await
            .unwrap();

            assert_eq!(result.total_decoded_blocks, 5);
            assert_eq!(result.total_imported_blocks, 5);

            // Verify stage checkpoints exist
            let provider = provider_factory.database_provider_ro().unwrap();
            let headers_checkpoint = provider.get_stage_checkpoint(StageId::Headers).unwrap();
            assert!(headers_checkpoint.is_some(), "Headers checkpoint should exist after import");
            assert_eq!(
                headers_checkpoint.unwrap().block_number,
                5,
                "Headers checkpoint should be at block 5"
            );
            drop(provider);

            // Properly close static files to release all file handles
            let static_file_provider = provider_factory.static_file_provider();
            drop(static_file_provider);

            drop(provider_factory);
            drop(db);
        }

        // Give the OS a moment to release file locks
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Now reopen the database and verify checkpoints are still there
        {
            let db_env = reth_db::init_db(&db_path, DatabaseArguments::default()).unwrap();
            let db = Arc::new(db_env);

            let provider_factory: ProviderFactory<
                NodeTypesWithDBAdapter<reth_node_ethereum::EthereumNode, Arc<DatabaseEnv>>,
            > = ProviderFactory::new(
                db,
                chain_spec.clone(),
                reth_provider::providers::StaticFileProvider::read_only(static_files_path, false)
                    .unwrap(),
            );

            let provider = provider_factory.database_provider_ro().unwrap();

            // Check that stage checkpoints are still present
            let headers_checkpoint = provider.get_stage_checkpoint(StageId::Headers).unwrap();
            assert!(
                headers_checkpoint.is_some(),
                "Headers checkpoint should still exist after reopening database"
            );
            assert_eq!(
                headers_checkpoint.unwrap().block_number,
                5,
                "Headers checkpoint should still be at block 5"
            );

            // Verify we can read blocks
            let block_5_hash = provider.block_hash(5).unwrap();
            assert!(block_5_hash.is_some(), "Block 5 should exist in database");
            assert_eq!(block_5_hash.unwrap(), test_blocks[4].hash(), "Block 5 hash should match");

            // Check all stage checkpoints
            debug!(target: "e2e::import", "All stage checkpoints after reopening:");
            for stage in StageId::ALL {
                let checkpoint = provider.get_stage_checkpoint(stage).unwrap();
                debug!(target: "e2e::import", "  Stage {stage:?}: {checkpoint:?}");
            }
        }
    }

    /// Helper to create test chain spec
    fn create_test_chain_spec() -> Arc<ChainSpec> {
        Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(
                    serde_json::from_str(include_str!("testsuite/assets/genesis.json")).unwrap(),
                )
                .london_activated()
                .shanghai_activated()
                .build(),
        )
    }

    /// Helper to setup test blocks and write to RLP
    async fn setup_test_blocks_and_rlp(
        chain_spec: &ChainSpec,
        block_count: u64,
        temp_dir: &Path,
    ) -> (Vec<SealedBlock>, PathBuf) {
        let test_blocks = generate_test_blocks(chain_spec, block_count);
        assert_eq!(
            test_blocks.len(),
            block_count as usize,
            "Should have generated expected blocks"
        );

        let rlp_path = temp_dir.join("test_chain.rlp");
        write_blocks_to_rlp(&test_blocks, &rlp_path).expect("Failed to write RLP data");

        let rlp_size = std::fs::metadata(&rlp_path).expect("RLP file should exist").len();
        debug!(target: "e2e::import", "Wrote RLP file with size: {rlp_size} bytes");

        (test_blocks, rlp_path)
    }

    #[tokio::test]
    async fn test_import_blocks_only() {
        // Tests just the block import functionality without full node setup
        reth_tracing::init_test_tracing();

        let chain_spec = create_test_chain_spec();
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let (test_blocks, rlp_path) =
            setup_test_blocks_and_rlp(&chain_spec, 10, temp_dir.path()).await;

        // Create a test database
        let datadir = temp_dir.path().join("datadir");
        std::fs::create_dir_all(&datadir).unwrap();
        let db_path = datadir.join("db");
        let db_env = reth_db::init_db(&db_path, DatabaseArguments::default()).unwrap();
        let db = Arc::new(reth_db::test_utils::TempDatabase::new(db_env, db_path));

        // Create static files path
        let static_files_path = datadir.join("static_files");

        // Create a provider factory
        let provider_factory: ProviderFactory<MockNodeTypesWithDB> = ProviderFactory::new(
            db.clone(),
            chain_spec.clone(),
            reth_provider::providers::StaticFileProvider::read_write(static_files_path).unwrap(),
        );

        // Initialize genesis
        reth_db_common::init::init_genesis(&provider_factory).unwrap();

        // Import the chain data
        let import_config = ImportConfig::default();
        let config = Config::default();
        let evm_config = reth_node_ethereum::EthEvmConfig::new(chain_spec.clone());
        // Use NoopConsensus to skip gas limit validation for test imports
        let consensus = reth_consensus::noop::NoopConsensus::arc();

        let result = import_blocks_from_file(
            &rlp_path,
            import_config,
            provider_factory.clone(),
            &config,
            evm_config,
            consensus,
        )
        .await
        .unwrap();

        debug!(target: "e2e::import",
            "Import result: decoded {} blocks, imported {} blocks",
            result.total_decoded_blocks, result.total_imported_blocks
        );

        // Verify the import was successful
        assert_eq!(result.total_decoded_blocks, 10);
        assert_eq!(result.total_imported_blocks, 10);
        assert_eq!(result.total_decoded_txns, 0);
        assert_eq!(result.total_imported_txns, 0);

        // Verify we can read the imported blocks
        let provider = provider_factory.database_provider_ro().unwrap();
        let latest_block = provider.last_block_number().unwrap();
        assert_eq!(latest_block, 10, "Should have imported up to block 10");

        let block_10_hash = provider.block_hash(10).unwrap().expect("Block 10 should exist");
        assert_eq!(block_10_hash, test_blocks[9].hash(), "Block 10 hash should match");
    }

    #[tokio::test]
    async fn test_import_with_node_integration() {
        // Tests the full integration with node setup, forkchoice updates, and syncing
        reth_tracing::init_test_tracing();

        let chain_spec = create_test_chain_spec();
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let (test_blocks, rlp_path) =
            setup_test_blocks_and_rlp(&chain_spec, 10, temp_dir.path()).await;

        // Create FCU data for the tip
        let tip = test_blocks.last().expect("Should have generated blocks");
        let fcu_path = temp_dir.path().join("test_fcu.json");
        std::fs::write(&fcu_path, create_fcu_json(tip).to_string())
            .expect("Failed to write FCU data");

        // Setup nodes with imported chain
        let result = setup_engine_with_chain_import(
            1,
            chain_spec,
            false,
            TreeConfig::default(),
            &rlp_path,
            |_| EthPayloadBuilderAttributes::default(),
        )
        .await
        .expect("Failed to setup nodes with chain import");

        // Load and apply forkchoice state
        let fcu_state = load_forkchoice_state(&fcu_path).expect("Failed to load forkchoice state");

        let node = &result.nodes[0];

        // Send forkchoice update to make the imported chain canonical
        node.update_forkchoice(fcu_state.finalized_block_hash, fcu_state.head_block_hash)
            .await
            .expect("Failed to update forkchoice");

        // Wait for the node to sync to the head
        node.sync_to(fcu_state.head_block_hash).await.expect("Failed to sync to head");

        // Verify the chain tip
        let latest = node
            .inner
            .provider
            .sealed_header_by_id(alloy_eips::BlockId::latest())
            .expect("Failed to get latest header")
            .expect("No latest header found");

        assert_eq!(
            latest.hash(),
            fcu_state.head_block_hash,
            "Chain tip does not match expected head"
        );
    }
}

//! Setup utilities for importing RLP chain data before starting nodes.

use crate::{node::NodeTestContext, NodeHelperType, Wallet};
use reth_chainspec::ChainSpec;
use reth_cli_commands::import_op::{import_blocks_from_file, ImportConfig};
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
#[allow(missing_debug_implementations)]
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

/// Creates a test setup with Ethereum nodes that have pre-imported chain data from RLP files.
///
/// This function:
/// 1. Creates a temporary datadir for each node
/// 2. Imports the specified RLP chain data into the datadir
/// 3. Starts the nodes with the pre-populated database
/// 4. Returns the running nodes ready for testing
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
        type ImportNodeTypes = reth_node_ethereum::EthereumNode;
        let provider_factory = ProviderFactory::<
            NodeTypesWithDBAdapter<ImportNodeTypes, Arc<DatabaseEnv>>,
        >::new(
            db.clone(),
            chain_spec.clone(),
            reth_provider::providers::StaticFileProvider::read_write(static_files_path.clone())?,
        );

        // Initialize genesis if needed
        reth_db_common::init::init_genesis(&provider_factory)?;

        // Import the chain data
        let import_config = ImportConfig::default();
        let config = Config::default();

        // Create EVM and consensus for Ethereum
        let evm_config = reth_node_ethereum::EthEvmConfig::new(chain_spec.clone());
        let consensus = reth_ethereum_consensus::EthBeaconConsensus::new(chain_spec.clone());

        let result = import_blocks_from_file(
            rlp_path,
            import_config,
            provider_factory.clone(),
            &config,
            evm_config,
            Arc::new(consensus),
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

        // The import counts genesis block in total_imported_blocks, so we expect
        // total_imported_blocks to be total_decoded_blocks + 1
        let expected_imported = result.total_decoded_blocks + 1; // +1 for genesis
        if result.total_imported_blocks != expected_imported {
            debug!(target: "e2e::import",
                "Import block count mismatch: expected {} (decoded {} + genesis), got {}",
                expected_imported, result.total_decoded_blocks, result.total_imported_blocks
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
        wallet: crate::Wallet::default().with_chain_id(chain_spec.chain().into()),
        _temp_dirs: temp_dirs,
    })
}

/// Helper to load forkchoice state from a JSON file
pub fn load_forkchoice_state(path: &Path) -> eyre::Result<alloy_rpc_types_engine::ForkchoiceState> {
    let json_str = std::fs::read_to_string(path)?;
    let fcu_data: serde_json::Value = serde_json::from_str(&json_str)?;

    let state = &fcu_data["forkchoiceState"];
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
    use alloy_consensus::{constants::EMPTY_WITHDRAWALS, BlockHeader, Header};
    use alloy_eips::eip4895::Withdrawals;
    use alloy_primitives::{Address, B256, B64, U256};
    use reth_chainspec::{ChainSpecBuilder, EthereumHardforks, MAINNET};
    use reth_db::mdbx::DatabaseArguments;
    use reth_ethereum_primitives::{Block, BlockBody};
    use reth_payload_builder::EthPayloadBuilderAttributes;
    use reth_primitives::SealedBlock;
    use reth_primitives_traits::Block as BlockTrait;
    use reth_provider::{
        test_utils::MockNodeTypesWithDB, BlockHashReader, BlockNumReader, BlockReaderIdExt,
    };
    use std::io::Write;

    /// Generate test blocks for a given chain spec
    fn generate_test_blocks(chain_spec: &ChainSpec, count: u64) -> Vec<SealedBlock> {
        let mut blocks: Vec<SealedBlock> = Vec::new();
        let genesis_header = chain_spec.sealed_genesis_header();
        let mut parent_hash = genesis_header.hash();
        let mut parent_number = genesis_header.number();
        let mut parent_base_fee = genesis_header.base_fee_per_gas;
        let mut parent_gas_limit = genesis_header.gas_limit;
        debug!(target: "e2e::import",
            "Genesis header base fee: {:?}, gas limit: {}, state root: {:?}",
            parent_base_fee,
            parent_gas_limit,
            genesis_header.state_root()
        );

        for i in 1..=count {
            // Create a simple header
            let mut header = Header {
                parent_hash,
                number: parent_number + 1,
                gas_limit: parent_gas_limit, // Use parent's gas limit
                gas_used: 0,                 // Empty blocks use no gas
                timestamp: genesis_header.timestamp() + i * 12, // 12 second blocks
                beneficiary: Address::ZERO,
                receipts_root: alloy_consensus::constants::EMPTY_RECEIPTS,
                logs_bloom: Default::default(),
                difficulty: U256::from(1), // Will be overridden for post-merge
                // Use the same state root as parent for now (empty state changes)
                state_root: if i == 1 {
                    genesis_header.state_root()
                } else {
                    blocks.last().unwrap().state_root
                },
                transactions_root: alloy_consensus::constants::EMPTY_TRANSACTIONS,
                ommers_hash: alloy_consensus::constants::EMPTY_OMMER_ROOT_HASH,
                mix_hash: B256::ZERO,
                nonce: B64::from(0u64),
                extra_data: Default::default(),
                base_fee_per_gas: None,
                withdrawals_root: None,
                blob_gas_used: None,
                excess_blob_gas: None,
                parent_beacon_block_root: None,
                requests_hash: None,
            };

            // Set required fields based on chain spec
            if chain_spec.is_london_active_at_block(header.number) {
                // Calculate base fee based on parent block
                if let Some(parent_fee) = parent_base_fee {
                    // For the first block, we need to use the exact expected base fee
                    // The consensus rules expect it to be calculated from the genesis
                    let (parent_gas_used, parent_gas_limit) = if i == 1 {
                        // Genesis block parameters
                        (genesis_header.gas_used, genesis_header.gas_limit)
                    } else {
                        let last_block = blocks.last().unwrap();
                        (last_block.gas_used, last_block.gas_limit)
                    };
                    header.base_fee_per_gas = Some(alloy_eips::calc_next_block_base_fee(
                        parent_gas_used,
                        parent_gas_limit,
                        parent_fee,
                        chain_spec.base_fee_params_at_timestamp(header.timestamp),
                    ));
                    debug!(target: "e2e::import", "Block {} calculated base fee: {:?} (parent gas used: {}, parent gas limit: {}, parent base fee: {})",
                        i, header.base_fee_per_gas, parent_gas_used, parent_gas_limit, parent_fee);
                    parent_base_fee = header.base_fee_per_gas;
                }
            }

            // For post-merge blocks
            if chain_spec.is_paris_active_at_block(header.number) {
                header.difficulty = U256::ZERO;
                header.nonce = B64::ZERO;
            }

            // For post-shanghai blocks
            if chain_spec.is_shanghai_active_at_timestamp(header.timestamp) {
                header.withdrawals_root = Some(EMPTY_WITHDRAWALS);
            }

            // For post-cancun blocks
            if chain_spec.is_cancun_active_at_timestamp(header.timestamp) {
                header.blob_gas_used = Some(0);
                header.excess_blob_gas = Some(0);
                header.parent_beacon_block_root = Some(B256::ZERO);
            }

            // Create an empty block body
            let body = BlockBody {
                transactions: vec![],
                ommers: vec![],
                withdrawals: header.withdrawals_root.is_some().then(Withdrawals::default),
            };

            // Create the block
            let block = Block { header: header.clone(), body: body.clone() };
            let sealed_block = BlockTrait::seal_slow(block);

            debug!(target: "e2e::import",
                "Generated block {} with hash {:?}",
                sealed_block.number(),
                sealed_block.hash()
            );
            debug!(target: "e2e::import",
                "  Body has {} transactions, {} ommers, withdrawals: {}",
                body.transactions.len(),
                body.ommers.len(),
                body.withdrawals.is_some()
            );

            // Update parent for next iteration
            parent_hash = sealed_block.hash();
            parent_number = sealed_block.number();
            parent_gas_limit = sealed_block.gas_limit;
            if header.base_fee_per_gas.is_some() {
                parent_base_fee = header.base_fee_per_gas;
            }

            blocks.push(sealed_block);
        }

        blocks
    }

    /// Write blocks to RLP file
    fn write_blocks_to_rlp(blocks: &[SealedBlock], path: &Path) -> std::io::Result<()> {
        use alloy_rlp::Encodable;

        let mut file = std::fs::File::create(path)?;
        let mut total_bytes = 0;

        for (i, block) in blocks.iter().enumerate() {
            // Convert SealedBlock to Block before encoding
            let block_for_encoding = block.clone().unseal();

            let mut buf = Vec::new();
            block_for_encoding.encode(&mut buf);
            debug!(target: "e2e::import",
                "Block {} has {} transactions, encoded to {} bytes",
                i,
                block.body().transactions.len(),
                buf.len()
            );

            // Debug: check what's in the encoded data
            debug!(target: "e2e::import", "Block {} encoded to {} bytes", i, buf.len());
            if buf.len() < 20 {
                debug!(target: "e2e::import", "  Raw bytes: {:?}", &buf);
            } else {
                debug!(target: "e2e::import", "  First 20 bytes: {:?}", &buf[..20]);
            }

            total_bytes += buf.len();
            file.write_all(&buf)?;
        }

        file.flush()?;
        debug!(target: "e2e::import", "Total RLP bytes written: {total_bytes}");
        Ok(())
    }

    /// Create FCU JSON for the tip of the chain
    fn create_fcu_json(tip: &SealedBlock) -> serde_json::Value {
        serde_json::json!({
            "forkchoiceState": {
                "headBlockHash": format!("0x{:x}", tip.hash()),
                "safeBlockHash": format!("0x{:x}", tip.hash()),
                "finalizedBlockHash": format!("0x{:x}", tip.hash()),
            }
        })
    }

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
            let consensus = reth_ethereum_consensus::EthBeaconConsensus::new(chain_spec.clone());

            let result = import_blocks_from_file(
                &rlp_path,
                import_config,
                provider_factory.clone(),
                &config,
                evm_config,
                Arc::new(consensus),
            )
            .await
            .unwrap();

            assert_eq!(result.total_decoded_blocks, 5);
            assert_eq!(result.total_imported_blocks, 6); // +1 for genesis

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
    ) -> (Vec<SealedBlock>, std::path::PathBuf) {
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
        let consensus = reth_ethereum_consensus::EthBeaconConsensus::new(chain_spec.clone());

        let result = import_blocks_from_file(
            &rlp_path,
            import_config,
            provider_factory.clone(),
            &config,
            evm_config,
            Arc::new(consensus),
        )
        .await
        .unwrap();

        debug!(target: "e2e::import",
            "Import result: decoded {} blocks, imported {} blocks",
            result.total_decoded_blocks, result.total_imported_blocks
        );

        // Verify the import was successful
        assert_eq!(result.total_decoded_blocks, 10);
        assert_eq!(result.total_imported_blocks, 11); // +1 for genesis
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

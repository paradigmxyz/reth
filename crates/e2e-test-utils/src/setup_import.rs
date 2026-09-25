//! Setup utilities for importing RLP chain data before starting nodes.

use crate::{
    eth_payload_attributes,
    setup_builder::{launch_test_node, test_node_config, LaunchArgs},
    wallet::Wallet,
    NodeHelperType,
};
use eyre::WrapErr;
use reth_chainspec::ChainSpec;
use reth_cli_commands::import_core::{import_blocks_from_file, ImportConfig, ImportResult};
use reth_config::Config;
use reth_db::DatabaseEnv;
use reth_node_api::{NodeTypesWithDBAdapter, TreeConfig};
use reth_node_core::args::StorageArgs;
use reth_node_ethereum::EthereumNode;
use reth_provider::{
    providers::{RocksDBProvider, StaticFileProvider},
    DatabaseProviderFactory, ProviderFactory, StageCheckpointReader, StorageSettings,
};
use reth_stages_types::StageId;
use reth_tasks::Runtime;
use std::{path::Path, sync::Arc};
use tempfile::TempDir;
use tracing::{debug, info, span, Instrument, Level};

/// Setup result containing nodes and temporary directories that must be kept alive
pub struct ChainImportResult {
    /// The nodes that were created
    pub nodes: Vec<NodeHelperType<EthereumNode>>,
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
/// The import and all nodes share a single [`Runtime::test`] runtime, and the nodes build payloads
/// with [`eth_payload_attributes`] for the given chain spec. `storage_v2` selects the storage
/// layout (`--storage.v2`) for both the imported database and the launched nodes.
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
    storage_v2: bool,
    tree_config: TreeConfig,
    rlp_path: &Path,
) -> eyre::Result<ChainImportResult> {
    let runtime = Runtime::test();
    let attributes_generator = {
        let chain_spec = chain_spec.clone();
        Arc::new(move |timestamp| eth_payload_attributes(&chain_spec, timestamp))
    };

    // Create nodes with imported data.
    let mut nodes = Vec::with_capacity(num_nodes);
    // Keep the temp dirs alive for the lifetime of the nodes.
    let mut temp_dirs = Vec::with_capacity(num_nodes);

    for idx in 0..num_nodes {
        // Create a temporary datadir for this node.
        let temp_dir = TempDir::new()?;
        let datadir = temp_dir.path().to_path_buf();
        debug!(target: "e2e::import", "Node {idx} datadir: {datadir:?}");

        let span = span!(Level::INFO, "node", idx);
        let node_config = test_node_config(chain_spec.clone())
            .set_dev(is_dev)
            .with_storage(StorageArgs { v2: storage_v2 });

        // First, import the chain data into this datadir.
        import_chain(
            &datadir,
            chain_spec.clone(),
            node_config.storage_settings(),
            rlp_path,
            runtime.clone(),
        )
        .instrument(span.clone())
        .await
        .wrap_err_with(|| format!("chain import failed for node {idx}"))?;

        // Now launch the node with the pre-populated datadir.
        debug!(target: "e2e::import", "Launching node with datadir: {:?}", datadir);

        let node = launch_test_node::<EthereumNode>(LaunchArgs {
            node_config,
            runtime: runtime.clone(),
            tree_config: tree_config.clone(),
            datadir,
            attributes_generator: attributes_generator.clone(),
            dev_payload_attributes: None,
        })
        .instrument(span)
        .await?;

        nodes.push(node);
        temp_dirs.push(temp_dir);
    }

    Ok(ChainImportResult {
        nodes,
        wallet: Wallet::default().with_chain_id(chain_spec.chain.id()),
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

/// Initializes the database in `datadir` with the given storage settings and imports the RLP
/// encoded chain at `rlp_path`.
///
/// All database handles are released on return so the datadir can be reopened, e.g. by a node,
/// which reads the storage settings back from the database.
async fn import_chain(
    datadir: &Path,
    chain_spec: Arc<ChainSpec>,
    storage_settings: StorageSettings,
    rlp_path: &Path,
    runtime: Runtime,
) -> eyre::Result<ImportResult> {
    info!(target: "test", "Importing chain data from {:?} into {:?}", rlp_path, datadir);

    // Initialize the database using init_db, same as the CLI import command.
    let db_args = reth_node_core::args::DatabaseArgs::default().database_args();
    let db = reth_db::init_db(datadir.join("db"), db_args)?;

    // Create a provider factory with the initialized database, not a `TempDatabase`.
    let provider_factory =
        ProviderFactory::<NodeTypesWithDBAdapter<EthereumNode, DatabaseEnv>>::new(
            db.clone(),
            chain_spec.clone(),
            StaticFileProvider::read_write(datadir.join("static_files"))?,
            RocksDBProvider::builder(datadir.join("rocksdb")).with_default_tables().build()?,
            runtime.clone(),
        )?;

    // Initialize genesis with the storage settings of the node that later opens the database.
    reth_db_common::init::init_genesis_with_settings(&provider_factory, storage_settings)?;

    // Use NoopConsensus to skip gas limit validation for test imports.
    let result = import_blocks_from_file(
        rlp_path,
        ImportConfig::default(),
        provider_factory.clone(),
        &Config::default(),
        reth_node_ethereum::EthEvmConfig::new(chain_spec),
        reth_consensus::noop::NoopConsensus::arc(),
        runtime,
    )
    .await?;

    info!(
        target: "test",
        "Imported {} blocks and {} transactions",
        result.total_imported_blocks,
        result.total_imported_txns,
    );

    debug!(target: "e2e::import",
        "Import result: decoded {} blocks, imported {} blocks, complete: {}",
        result.total_decoded_blocks,
        result.total_imported_blocks,
        result.is_complete()
    );

    eyre::ensure!(
        result.total_decoded_blocks == result.total_imported_blocks,
        "block count mismatch: decoded {} != imported {}",
        result.total_decoded_blocks,
        result.total_imported_blocks
    );
    eyre::ensure!(
        result.total_decoded_txns == result.total_imported_txns,
        "transaction count mismatch: decoded {} != imported {}",
        result.total_decoded_txns,
        result.total_imported_txns
    );

    // Verify the database was properly initialized by checking stage checkpoints.
    let headers_checkpoint =
        provider_factory.database_provider_ro()?.get_stage_checkpoint(StageId::Headers)?;
    eyre::ensure!(headers_checkpoint.is_some(), "Headers stage checkpoint is missing after import");
    debug!(target: "e2e::import", "Headers stage checkpoint after import: {headers_checkpoint:?}");

    // Close all database handles to release locks before launching the node.
    drop(provider_factory);
    drop(db);

    // The header and body downloader tasks spawned on the runtime hold provider factory clones
    // and only exit once they are polled after the pipeline was dropped, so yield long enough for
    // them to release the database.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_chain_spec,
        test_rlp_utils::{create_fcu_json, generate_test_blocks, write_blocks_to_rlp},
    };
    use reth_chainspec::EthereumHardfork;
    use reth_db::mdbx::DatabaseArguments;
    use reth_ethereum_primitives::Block;
    use reth_primitives_traits::SealedBlock;
    use reth_provider::{BlockHashReader, BlockNumReader, BlockReaderIdExt, MetadataProvider};
    use std::path::PathBuf;

    /// Helper to setup test blocks and write to RLP.
    fn setup_test_blocks_and_rlp(
        chain_spec: &ChainSpec,
        block_count: u64,
        temp_dir: &Path,
    ) -> (Vec<SealedBlock<Block>>, PathBuf) {
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

    /// Reopens the database in `datadir` that was written by [`import_chain`].
    fn reopen_provider_factory(
        datadir: &Path,
        chain_spec: Arc<ChainSpec>,
        runtime: Runtime,
    ) -> ProviderFactory<NodeTypesWithDBAdapter<EthereumNode, DatabaseEnv>> {
        let db = reth_db::init_db(datadir.join("db"), DatabaseArguments::default()).unwrap();
        ProviderFactory::new(
            db,
            chain_spec,
            StaticFileProvider::read_only(datadir.join("static_files")).unwrap(),
            RocksDBProvider::builder(datadir.join("rocksdb"))
                .with_default_tables()
                .build()
                .unwrap(),
            runtime,
        )
        .expect("failed to create provider factory")
    }

    #[tokio::test]
    async fn test_import_blocks_and_reopen() {
        // Tests the block import without a node, and that the imported chain and stage
        // checkpoints persist when reopening the database.
        reth_tracing::init_test_tracing();

        let runtime = Runtime::test();
        let chain_spec = test_chain_spec(EthereumHardfork::Shanghai);
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let (test_blocks, rlp_path) = setup_test_blocks_and_rlp(&chain_spec, 10, temp_dir.path());

        let datadir = temp_dir.path().join("datadir");
        std::fs::create_dir_all(&datadir).unwrap();
        let storage_settings = StorageSettings { storage_v2: StorageArgs::default().v2 };
        let result = import_chain(
            &datadir,
            chain_spec.clone(),
            storage_settings,
            &rlp_path,
            runtime.clone(),
        )
        .await
        .unwrap();
        assert_eq!(result.total_decoded_blocks, 10);
        assert_eq!(result.total_imported_blocks, 10);
        assert_eq!(result.total_decoded_txns, 0);
        assert_eq!(result.total_imported_txns, 0);

        let provider_factory = reopen_provider_factory(&datadir, chain_spec, runtime);
        let provider = provider_factory.database_provider_ro().unwrap();
        assert_eq!(provider.last_block_number().unwrap(), 10);
        assert_eq!(provider.block_hash(10).unwrap(), Some(test_blocks[9].hash()));
        let headers_checkpoint = provider.get_stage_checkpoint(StageId::Headers).unwrap();
        assert_eq!(headers_checkpoint.map(|checkpoint| checkpoint.block_number), Some(10));
    }

    #[tokio::test]
    async fn test_import_with_node_integration() {
        // Tests the full integration with node setup, forkchoice updates, and syncing.
        reth_tracing::init_test_tracing();

        let chain_spec = test_chain_spec(EthereumHardfork::Shanghai);
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let (test_blocks, rlp_path) = setup_test_blocks_and_rlp(&chain_spec, 10, temp_dir.path());

        // Create FCU data for the tip.
        let tip = test_blocks.last().expect("Should have generated blocks");
        let fcu_path = temp_dir.path().join("test_fcu.json");
        std::fs::write(&fcu_path, create_fcu_json(tip).to_string())
            .expect("Failed to write FCU data");

        for storage_v2 in [false, true] {
            // Setup nodes with imported chain.
            let result = setup_engine_with_chain_import(
                1,
                chain_spec.clone(),
                false,
                storage_v2,
                TreeConfig::default(),
                &rlp_path,
            )
            .await
            .expect("Failed to setup nodes with chain import");

            // Load and apply forkchoice state.
            let fcu_state =
                load_forkchoice_state(&fcu_path).expect("Failed to load forkchoice state");

            let node = &result.nodes[0];

            // The imported database and the node use the requested storage layout.
            let settings = node
                .inner
                .provider
                .database_provider_ro()
                .expect("Failed to open database provider")
                .storage_settings()
                .expect("Failed to read storage settings");
            assert_eq!(settings, Some(StorageSettings { storage_v2 }));

            // Send forkchoice update to make the imported chain canonical.
            node.update_forkchoice(fcu_state.finalized_block_hash, fcu_state.head_block_hash)
                .await
                .expect("Failed to update forkchoice");

            // Wait for the node to sync to the head.
            node.sync_to(fcu_state.head_block_hash).await.expect("Failed to sync to head");

            // Verify the chain tip.
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
}

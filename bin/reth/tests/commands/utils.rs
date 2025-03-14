//!
//! Utils for bitfinity integration tests
//!
use std::{
    fmt::{Debug, Display, Formatter},
    path::PathBuf,
    str::FromStr,
    sync::Arc,
    time,
};

use alloy_eips::eip7685::Requests;
use alloy_primitives::BlockNumber;
use lightspeed_scheduler::JobExecutor;
use parking_lot::Mutex;
use reth::{
    args::{BitfinityImportArgs, IC_MAINNET_KEY},
    commands::bitfinity_import::BitfinityImportCommand,
    dirs::{ChainPath, DataDirPath, PlatformPath},
};
use reth_beacon_consensus::EthBeaconConsensus;
use reth_blockchain_tree::{BlockchainTreeConfig, ShareableBlockchainTree, TreeExternals};
use reth_chainspec::ChainSpec;
use reth_db::{init_db, DatabaseEnv};
use reth_downloaders::bitfinity_evm_client::BitfinityEvmClient;
use reth_errors::BlockExecutionError;
use reth_evm::execute::{BatchExecutor, BlockExecutionOutput, BlockExecutorProvider, Executor};
use reth_node_api::NodeTypesWithDBAdapter;
use reth_node_ethereum::EthereumNode;
use reth_primitives::{BlockWithSenders, EthPrimitives, Receipt};
use reth_provider::{
    providers::{BlockchainProvider, StaticFileProvider},
    BlockNumReader, ExecutionOutcome, ProviderError, ProviderFactory,
};
use reth_prune::PruneModes;
use reth_tracing::{FileWorkerGuard, LayerInfo, LogFormat, RethTracer, Tracer};
use revm_primitives::db::Database;
use tempfile::TempDir;
use tracing::{debug, info};

/// Local EVM canister ID for testing.
pub const LOCAL_EVM_CANISTER_ID: &str = "bkyz2-fmaaa-aaaaa-qaaaq-cai";

/// EVM block extractor for devnet running on Digital Ocean.
pub const DEFAULT_EVM_DATASOURCE_URL: &str =
    "https://block-extractor-testnet-1052151659755.europe-west9.run.app";

/// Initializes the logs for the tests.
pub fn init_logs() -> eyre::Result<Option<FileWorkerGuard>> {
    let mut tracer = RethTracer::new();
    let stdout = LayerInfo::new(
        LogFormat::Terminal,
        "debug".to_string(),
        String::new(),
        Some("always".to_string()),
    );
    tracer = tracer.with_stdout(stdout);

    let guard = tracer.init()?;
    Ok(guard)
}

/// Type alias for the node types.
pub type NodeTypes = NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>;

#[derive(Clone)]
/// Data needed for the import tests.
pub struct ImportData {
    /// The chain spec.
    pub chain: Arc<ChainSpec>,
    /// The data directory.
    pub data_dir: ChainPath<DataDirPath>,
    /// The database.
    pub database: Arc<DatabaseEnv>,
    /// The provider factory.
    pub provider_factory: ProviderFactory<NodeTypes>,
    /// The blockchain provider.
    pub blockchain_db: BlockchainProvider<NodeTypes>,
    /// The bitfinity import arguments.
    pub bitfinity_args: BitfinityImportArgs,
}

impl Debug for ImportData {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportTempData").field("chain", &self.chain).finish()
    }
}

/// Imports blocks from the EVM node.
pub async fn import_blocks(
    import_data: ImportData,
    timeout: time::Duration,
    shutdown_when_done: bool,
) -> JobExecutor {
    let end_block = import_data.bitfinity_args.end_block.unwrap();
    let import = BitfinityImportCommand::new(
        None,
        import_data.data_dir,
        import_data.chain,
        import_data.bitfinity_args,
        import_data.provider_factory.clone(),
        import_data.blockchain_db,
    );
    let (job_executor, _import_handle) = import.schedule_execution().await.unwrap();
    wait_until_local_block_imported(&import_data.provider_factory, end_block, timeout).await;

    if shutdown_when_done {
        debug!("Stopping job executor");
        job_executor.stop(true).await.unwrap();
        debug!("Job executor stopped");
    }

    job_executor
}

/// Initializes the database and the blockchain tree for the bitfinity import tests.
/// If a `data_dir` is provided, it will be used, otherwise a temporary directory will be created.
pub async fn bitfinity_import_config_data(
    evm_datasource_url: &str,
    backup_evm_datasource_url: Option<String>,
    data_dir: Option<PathBuf>,
) -> eyre::Result<(TempDir, ImportData)> {
    let chain = Arc::new(
        BitfinityEvmClient::fetch_chain_spec_with_fallback(
            evm_datasource_url.to_owned(),
            backup_evm_datasource_url.clone(),
        )
        .await?,
    );

    let temp_dir = TempDir::new().unwrap();

    let data_dir = data_dir.unwrap_or_else(|| temp_dir.path().to_path_buf());
    let data_dir: PlatformPath<DataDirPath> =
        PlatformPath::from_str(data_dir.as_os_str().to_str().unwrap())?;
    let data_dir = ChainPath::new(data_dir, chain.chain, Default::default());

    let db_path = data_dir.db();

    let database = Arc::new(init_db(db_path, Default::default())?);
    let provider_factory = ProviderFactory::new(
        database.clone(),
        chain.clone(),
        StaticFileProvider::read_write(data_dir.static_files())?,
    );

    reth_db_common::init::init_genesis(&provider_factory)?;

    let consensus = Arc::new(EthBeaconConsensus::new(chain.clone()));

    let executor = MockExecutorProvider::default(); //EvmExecutorFac::new(self.chain.clone(), EthEvmConfig::default());

    let blockchain_tree =
        Arc::new(ShareableBlockchainTree::new(reth_blockchain_tree::BlockchainTree::new(
            TreeExternals::new(provider_factory.clone(), consensus, executor),
            BlockchainTreeConfig::default(),
        )?));

    let blockchain_db = BlockchainProvider::new(provider_factory.clone(), blockchain_tree)?;

    let bitfinity_args = BitfinityImportArgs {
        rpc_url: evm_datasource_url.to_string(),
        send_raw_transaction_rpc_url: None,
        end_block: Some(100),
        import_interval: 1,
        batch_size: 1000,
        max_fetch_blocks: 10000,
        evmc_principal: LOCAL_EVM_CANISTER_ID.to_string(),
        ic_root_key: IC_MAINNET_KEY.to_string(),
        backup_rpc_url: backup_evm_datasource_url,
        max_retries: 3,
        retry_delay_secs: 3,
        check_evm_state_before_importing: false,
        max_block_age_secs: 600,
        confirm_unsafe_blocks: false,
    };

    Ok((
        temp_dir,
        ImportData { data_dir, database, chain, provider_factory, blockchain_db, bitfinity_args },
    ))
}

/// Waits until the block is imported.
pub async fn wait_until_local_block_imported(
    provider_factory: &ProviderFactory<NodeTypes>,
    block: BlockNumber,
    timeout: time::Duration,
) {
    let now = std::time::Instant::now();
    loop {
        let provider = provider_factory.provider().unwrap();
        let last_block = provider.last_block_number().unwrap();
        if last_block == block {
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        assert!(now.elapsed() <= timeout, "Timeout waiting for the last block to be imported. Waiting for block: {} but last block found was {}", block, last_block)
    }
}

/// Returns the local dfx port.
pub fn get_dfx_local_port() -> u16 {
    use std::process::Command;
    let output = Command::new("dfx")
        .arg("info")
        .arg("replica-port")
        .output()
        .expect("failed to execute process");

    let port = String::from_utf8_lossy(&output.stdout);
    info!("dfx port: {}", port);
    u16::from_str(port.trim()).unwrap()
}

/// A [`BlockExecutorProvider`] that returns mocked execution results.
/// Original code taken from ./`crates/evm/src/test_utils.rs`
#[derive(Clone, Debug, Default)]
pub struct MockExecutorProvider {
    exec_results: Arc<Mutex<Vec<ExecutionOutcome>>>,
}

impl BlockExecutorProvider for MockExecutorProvider {
    type Executor<DB: Database<Error: Into<ProviderError> + Display>> = Self;

    type BatchExecutor<DB: Database<Error: Into<ProviderError> + Display>> = Self;

    fn executor<DB>(&self, _: DB) -> Self::Executor<DB>
    where
        DB: Database<Error: Into<ProviderError> + Display>,
    {
        self.clone()
    }

    fn batch_executor<DB>(&self, _: DB) -> Self::BatchExecutor<DB>
    where
        DB: Database<Error: Into<ProviderError> + Display>,
    {
        self.clone()
    }

    type Primitives = EthPrimitives;
}

impl<DB> Executor<DB> for MockExecutorProvider {
    type Input<'a> = &'a BlockWithSenders;
    type Output = BlockExecutionOutput<Receipt>;
    type Error = BlockExecutionError;

    fn execute(self, _: Self::Input<'_>) -> Result<Self::Output, Self::Error> {
        let ExecutionOutcome { bundle, receipts, requests, first_block: _ } =
            self.exec_results.lock().pop().unwrap();
        Ok(BlockExecutionOutput {
            state: bundle,
            receipts: receipts.into_iter().flatten().flatten().collect(),
            requests: Requests::new(requests.into_iter().flatten().collect()),
            gas_used: 0,
        })
    }

    fn execute_with_state_closure<F>(
        self,
        _input: Self::Input<'_>,
        _state: F,
    ) -> Result<Self::Output, Self::Error>
    where
        F: FnMut(&reth_revm::State<DB>),
    {
        let ExecutionOutcome { bundle, receipts, requests, first_block: _ } =
            self.exec_results.lock().pop().unwrap();
        Ok(BlockExecutionOutput {
            state: bundle,
            receipts: receipts.into_iter().flatten().flatten().collect(),
            requests: Requests::new(requests.into_iter().flatten().collect()),
            gas_used: 0,
        })
    }

    fn execute_with_state_hook<F>(
        self,
        _input: Self::Input<'_>,
        _state_hook: F,
    ) -> Result<Self::Output, Self::Error>
    where
        F: reth_evm::system_calls::OnStateHook + 'static,
    {
        todo!()
    }
}

impl<DB> BatchExecutor<DB> for MockExecutorProvider {
    type Input<'a> = &'a BlockWithSenders;
    type Output = ExecutionOutcome;
    type Error = BlockExecutionError;

    fn execute_and_verify_one(&mut self, _: Self::Input<'_>) -> Result<(), Self::Error> {
        Ok(())
    }

    fn finalize(self) -> Self::Output {
        self.exec_results.lock().pop().unwrap()
    }

    fn set_tip(&mut self, _: BlockNumber) {}

    fn size_hint(&self) -> Option<usize> {
        None
    }

    fn set_prune_modes(&mut self, _prune_modes: PruneModes) {}
}

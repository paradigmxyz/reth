use std::{
    fmt::{Debug, Formatter},
    path::PathBuf,
    str::FromStr,
    sync::{Arc, Mutex},
    time,
};

use lightspeed_scheduler::JobExecutor;
use reth::{
    args::{BitfinityImportArgs, IC_MAINNET_KEY},
    commands::bitfinity_import::BitfinityImportCommand,
    dirs::{ChainPath, DataDirPath, PlatformPath},
};
use reth_beacon_consensus::EthBeaconConsensus;
use reth_blockchain_tree::{BlockchainTreeConfig, ShareableBlockchainTree, TreeExternals};
use reth_db::{init_db, DatabaseEnv};
use reth_downloaders::bitfinity_evm_client::BitfinityEvmClient;
use reth_evm::execute::{
    BatchBlockExecutionOutput, BatchExecutor, BlockExecutionInput, BlockExecutionOutput,
    BlockExecutorProvider, Executor,
};
use reth_interfaces::executor::BlockExecutionError;
use reth_node_core::init::init_genesis;
use reth_primitives::{BlockNumber, BlockWithSenders, ChainSpec, PruneModes, Receipt};
use reth_provider::{
    providers::BlockchainProvider, BlockNumReader, ProviderError, ProviderFactory,
};
use reth_tracing::{FileWorkerGuard, LayerInfo, LogFormat, RethTracer, Tracer};
use revm_primitives::db::Database;
use tempfile::TempDir;
use tracing::{debug, info};

pub const LOCAL_EVM_CANISTER_ID: &str = "bkyz2-fmaaa-aaaaa-qaaaq-cai";
/// EVM block extractor for devnet running on Digital Ocean.
pub const DEFAULT_EVM_DATASOURCE_URL: &str = "https://orca-app-5yyst.ondigitalocean.app";


pub fn init_logs() -> eyre::Result<Option<FileWorkerGuard>> {
    let mut tracer = RethTracer::new();
    let stdout = LayerInfo::new(
        LogFormat::Terminal,
        "info".to_string(),
        "".to_string(),
        Some("always".to_string()),
    );
    tracer = tracer.with_stdout(stdout);

    let guard = tracer.init()?;
    Ok(guard)
}

#[derive(Clone)]
pub struct ImportData {
    pub chain: Arc<ChainSpec>,
    pub data_dir: ChainPath<DataDirPath>,
    pub provider_factory: ProviderFactory<Arc<DatabaseEnv>>,
    pub blockchain_db: BlockchainProvider<Arc<DatabaseEnv>>,
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
    end_block: BlockNumber,
    timeout: time::Duration,
    shutdown_when_done: bool,
) -> JobExecutor {
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
/// If a data_dir is provided, it will be used, otherwise a temporary directory will be created.
pub async fn bitfinity_import_config_data(
    evm_datasource_url: &str,
    data_dir: Option<PathBuf>,
) -> eyre::Result<(TempDir, ImportData)> {
    let chain =
        Arc::new(BitfinityEvmClient::fetch_chain_spec(evm_datasource_url.to_owned()).await?);

    let temp_dir = TempDir::new().unwrap();

    let data_dir = data_dir.unwrap_or(temp_dir.path().to_path_buf());
    let data_dir: PlatformPath<DataDirPath> =
        PlatformPath::from_str(data_dir.as_os_str().to_str().unwrap())?;
    let data_dir = ChainPath::new(data_dir, chain.chain.clone());

    let db_path = data_dir.db();

    let db = Arc::new(init_db(db_path, Default::default())?);
    let provider_factory =
        ProviderFactory::new(db.clone(), chain.clone(), data_dir.static_files())?;

    init_genesis(provider_factory.clone())?;

    let consensus = Arc::new(EthBeaconConsensus::new(chain.clone()));

    let executor = MockExecutorProvider::default(); //EvmExecutorFac::new(self.chain.clone(), EthEvmConfig::default());

    let blockchain_tree =
        Arc::new(ShareableBlockchainTree::new(reth_blockchain_tree::BlockchainTree::new(
            TreeExternals::new(provider_factory.clone(), consensus.clone(), executor),
            BlockchainTreeConfig::default(),
            None,
        )?));

    let blockchain_db = BlockchainProvider::new(provider_factory.clone(), blockchain_tree)?;

    // blockchain_db.update_chain_info()?;

    let bitfinity_args = BitfinityImportArgs {
        rpc_url: evm_datasource_url.to_string(),
        send_raw_transaction_rpc_url: None,
        end_block: Some(100),
        import_interval: 1,
        batch_size: 1000,
        evmc_principal: LOCAL_EVM_CANISTER_ID.to_string(),
        ic_root_key: IC_MAINNET_KEY.to_string(),
    };

    Ok((temp_dir, ImportData { data_dir, chain, provider_factory, blockchain_db, bitfinity_args }))
}

/// Waits until the block is imported.
pub async fn wait_until_local_block_imported(
    provider_factory: &ProviderFactory<Arc<DatabaseEnv>>,
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
        if now.elapsed() > timeout {
            panic!("Timeout waiting for the last block to be imported. Waiting for block: {} but last block found was {}", block, last_block);
        }
    }
}

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

/// A [BlockExecutorProvider] that returns mocked execution results.
#[derive(Clone, Debug, Default)]
struct MockExecutorProvider {
    exec_results: Arc<Mutex<Vec<BatchBlockExecutionOutput>>>,
}

impl BlockExecutorProvider for MockExecutorProvider {
    type Executor<DB: Database<Error = ProviderError>> = Self;

    type BatchExecutor<DB: Database<Error = ProviderError>> = Self;

    fn executor<DB>(&self, _: DB) -> Self::Executor<DB>
    where
        DB: Database<Error = ProviderError>,
    {
        self.clone()
    }

    fn batch_executor<DB>(&self, _: DB, _: PruneModes) -> Self::BatchExecutor<DB>
    where
        DB: Database<Error = ProviderError>,
    {
        self.clone()
    }
}

impl<DB> Executor<DB> for MockExecutorProvider {
    type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
    type Output = BlockExecutionOutput<Receipt>;
    type Error = BlockExecutionError;

    fn execute(self, _: Self::Input<'_>) -> Result<Self::Output, Self::Error> {
        let BatchBlockExecutionOutput { bundle, receipts, .. } =
            self.exec_results.lock().unwrap().pop().unwrap();
        Ok(BlockExecutionOutput {
            state: bundle,
            receipts: receipts.into_iter().flatten().flatten().collect(),
            gas_used: 0,
        })
    }
}

impl<DB> BatchExecutor<DB> for MockExecutorProvider {
    type Input<'a> = BlockExecutionInput<'a, BlockWithSenders>;
    type Output = BatchBlockExecutionOutput;
    type Error = BlockExecutionError;

    fn execute_one(&mut self, _: Self::Input<'_>) -> Result<(), Self::Error> {
        Ok(())
    }

    fn finalize(self) -> Self::Output {
        self.exec_results.lock().unwrap().pop().unwrap()
    }

    fn set_tip(&mut self, _: BlockNumber) {}

    fn size_hint(&self) -> Option<usize> {
        None
    }
}

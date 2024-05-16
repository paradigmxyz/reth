use std::{str::FromStr, sync::{Arc, Mutex}};

use reth::dirs::{ChainPath, PlatformPath};
use reth_beacon_consensus::EthBeaconConsensus;
use reth_blockchain_tree::{BlockchainTreeConfig, ShareableBlockchainTree, TreeExternals};
use reth_db::{ init_db, DatabaseEnv};
use reth_downloaders::bitfinity_evm_client::BitfinityEvmClient;
use reth_evm::execute::{BatchBlockExecutionOutput, BatchExecutor, BlockExecutionInput, BlockExecutionOutput, BlockExecutorProvider, Executor};
use reth_interfaces::executor::BlockExecutionError;
use reth_primitives::{BlockNumber, BlockWithSenders, ChainSpec, PruneModes, Receipt};
use reth_provider::{providers::BlockchainProvider, ProviderError, ProviderFactory};
use revm_primitives::db::Database;
use tempfile::TempDir;

pub async fn temp_data(
    evm_datasource_url: &str,
) -> eyre::Result<(TempDir, Arc<ChainSpec>, ProviderFactory<Arc<DatabaseEnv>>, BlockchainProvider<Arc<DatabaseEnv>>)> {
    
    let chain = Arc::new(BitfinityEvmClient::fetch_chain_spec(evm_datasource_url.to_owned()).await?);
    
    let temp_dir = TempDir::new().unwrap();
    let data_dir: PlatformPath<()> = PlatformPath::from_str(temp_dir.path().as_os_str().to_str().unwrap())?;
    let data_dir = ChainPath::new(data_dir, chain.chain.clone());

    let db_path = data_dir.db();

    let db = Arc::new(init_db(db_path, Default::default())?);
    let provider_factory = ProviderFactory::new(db.clone(), chain.clone(), data_dir.static_files())?;

    let consensus = Arc::new(EthBeaconConsensus::new(chain.clone()));
    
    let executor = MockExecutorProvider::default();//EvmExecutorFac::new(self.chain.clone(), EthEvmConfig::default());

    let blockchain_tree =
        Arc::new(ShareableBlockchainTree::new(reth_blockchain_tree::BlockchainTree::new(
            TreeExternals::new(provider_factory.clone(), consensus.clone(), executor),
            BlockchainTreeConfig::default(),
            None,
        )?));

    let blockchain_db = BlockchainProvider::new(provider_factory.clone(), blockchain_tree)?;

    // blockchain_db.update_chain_info()?;

    Ok((temp_dir, chain, provider_factory, blockchain_db))
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

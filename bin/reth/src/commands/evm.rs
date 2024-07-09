use clap::Parser;
use eyre::Result;
use tracing::{debug, info};
use crate::commands::common::{AccessRights, Environment, EnvironmentArgs};
use reth_db::{init_db, DatabaseEnv, tables, transaction::DbTx};
use reth_primitives::{BlockNumber, Header};
use std::sync::Arc;
use std::path::{Path, PathBuf};
use std::thread;
use std::thread::JoinHandle;
use reth_blockchain_tree::{BlockchainTree, BlockchainTreeConfig, ShareableBlockchainTree, TreeExternals};
use reth_blockchain_tree::noop::NoopBlockchainTree;
use reth_consensus::{Consensus, PostExecutionInput};
use reth_consensus_common::validation;
use reth_db_api::database::Database;
use reth_evm::execute::{BatchExecutor, BlockExecutionOutput, BlockExecutorProvider, Executor};
use reth_execution_types::ExecutionOutcome;
use reth_provider::{BlockReader, ChainSpecProvider, HeaderProvider, HistoricalStateProviderRef, LatestStateProviderRef, StaticFileProviderFactory};
use reth_provider::providers::BlockchainProvider;
use reth_prune_types::PruneModes;
use reth_revm::database::StateProviderDatabase;
use crate::beacon_consensus::EthBeaconConsensus;
use crate::macros::block_executor;
use crate::primitives::{BlockHashOrNumber, U256};
use crate::providers::{BlockNumReader, OriginalValuesKnown, ProviderError, StateProviderFactory, TransactionVariant};

/// EVM commands
#[derive(Debug, Parser)]
pub struct EvmCommand {
    #[command(flatten)]
    env: EnvironmentArgs,
    /// begin block number
    #[arg(long, alias = "begin", short = 'b')]
    begin_number: u64,
    /// end block number
    #[arg(long, alias = "end", short = 'e')]
    end_number: u64,
}

impl EvmCommand {
    /// Execute the `evm` command
    pub async fn execute(self) -> Result<()> {
        info!(target: "reth::cli", "Executing EVM command...");

        let Environment { provider_factory, config, data_dir } = self.env.init(AccessRights::RO)?;

        let consensus: Arc<dyn Consensus> =
            Arc::new(EthBeaconConsensus::new(provider_factory.chain_spec()));

        let executor = block_executor!(provider_factory.chain_spec());

        // configure blockchain tree
        let tree_externals =
            TreeExternals::new(provider_factory.clone(), Arc::clone(&consensus), executor);
        let tree = BlockchainTree::new(tree_externals, BlockchainTreeConfig::default(), None)?;
        let blockchain_tree = Arc::new(ShareableBlockchainTree::new(tree));
        let blockchain_db = BlockchainProvider::new(provider_factory.clone(), blockchain_tree.clone())?;

        let provider = provider_factory.provider()?;

        let last = provider.last_block_number()?;

        if self.begin_number > self.end_number {
            eyre::bail!("the begin block number is higher than the end block number")
        }
        if  self.end_number > last {
            eyre::bail!("The end block number is higher than the latest block number")
        }


        // 获取 CPU 核心数，减一作为线程数
        let cpu_count = self.get_cpu_count();
        let thread_count = cpu_count - 1;

        // 计算每个线程处理的区间大小
        let range_per_thread = (self.end_number - self.begin_number + 1) / thread_count as u64;

        let mut threads: Vec<JoinHandle<bool>> = Vec::with_capacity(thread_count);

        for i in 0..thread_count as u64 {
            let thread_start = self.begin_number + i * range_per_thread;
            let thread_end = if i == thread_count as u64 - 1 {
                self.end_number
            } else {
                thread_start + range_per_thread - 1
            };

            threads.push(thread::spawn(move || {

                let db = StateProviderDatabase::new(blockchain_db.history_by_block_number(thread_start-1)?);
                let executor = block_executor!(provider_factory.chain_spec()).executor(db);

                for target in thread_start..=thread_end {

                    let td = provider.header_td_by_number(target)?
                        .ok_or_else(|| ProviderError::HeaderNotFound(target.into()))?;

                    let block =  provider.sealed_block_with_senders(target.into(), TransactionVariant::WithHash)?
                        .ok_or_else(|| ProviderError::HeaderNotFound(target.into()))?;

                    let BlockExecutionOutput { state, receipts, requests, .. } =
                        executor.execute((&block.clone().unseal(),td).into())?;

                    let consensus: Arc<dyn Consensus> =
                        Arc::new(EthBeaconConsensus::new(provider_factory.chain_spec()));
                    consensus.validate_block_post_execution(&block.clone().unseal(), PostExecutionInput::new(&receipts, &requests))?;
                }
            });
        }

        threads.into_iter().all(|b| b.join().unwrap());

        Ok(())
    }

    // 获取系统 CPU 核心数
    fn get_cpu_count(&self) -> usize {
        num_cpus::get()
    }
}

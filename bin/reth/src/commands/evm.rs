use clap::Parser;
use eyre::Result;
use tracing::{debug, info};
use crate::commands::common::{AccessRights, Environment, EnvironmentArgs};
use reth_db::{init_db, DatabaseEnv, tables, transaction::DbTx};
use reth_primitives::{BlockNumber, Header};
use std::sync::{Arc, Mutex};
use std::path::{Path, PathBuf};
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;
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
use crate::providers::{BlockNumReader, OriginalValuesKnown, ProviderError, ProviderResult, StateProviderFactory, TransactionVariant};

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

        let Environment { provider_factory, .. } = self.env.init(AccessRights::RO)?;

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

        let mut threads: Vec<JoinHandle<Result<bool>>> = Vec::with_capacity(thread_count);


        // 创建共享 gas 计数器
        let cumulative_gas = Arc::new(Mutex::new(0));
        let block_counter = Arc::new(Mutex::new(0));

        // 创建状态输出线程
        {
            let cumulative_gas = Arc::clone(&cumulative_gas);
            let block_counter = Arc::clone(&block_counter);

            thread::spawn(move || {
                let mut previous_cumulative_gas:u64 = 0;
                let mut previous_block_counter:u64 = 0;
                loop {
                    thread::sleep(Duration::from_secs(1));

                    let current_cumulative_gas =  cumulative_gas.lock().unwrap();
                    let diff = *current_cumulative_gas - previous_cumulative_gas;
                    previous_cumulative_gas = current_cumulative_gas.clone();
                    let diff_in_g = diff as f64 / 1_000_000_000_000.0;

                    let current_block_counter =  block_counter.lock().unwrap();
                    let diff_block = *current_block_counter - previous_block_counter;
                    previous_block_counter = current_block_counter.clone();
                    info!(target: "reth::cli", "Processed gas: {} G block: {}", diff_in_g, diff_block);
                }
            });
        }

        for i in 0..thread_count as u64 {
            let thread_start = self.begin_number + i * range_per_thread;
            let thread_end = if i == thread_count as u64 - 1 {
                self.end_number
            } else {
                thread_start + range_per_thread - 1
            };

            let cumulative_gas = Arc::clone(&cumulative_gas);
            let block_counter = Arc::clone(&block_counter);

            let provider_factory = provider_factory.clone();
            let blockchain_db = blockchain_db.clone();
            let db = StateProviderDatabase::new(blockchain_db.history_by_block_number(thread_start-1)?);
            let executor = block_executor!(provider_factory.chain_spec()).clone();

            let mut executor = executor.batch_executor(db, PruneModes::none());

            threads.push(thread::spawn(move || {
                for target in thread_start..=thread_end {
                    let td = blockchain_db.header_td_by_number(target)?
                        .ok_or_else(|| ProviderError::HeaderNotFound(target.into()))?;

                    let block =  blockchain_db.sealed_block_with_senders(target.into(), TransactionVariant::WithHash)?
                        .ok_or_else(|| ProviderError::HeaderNotFound(target.into()))?;

                    executor.execute_and_verify_one((&block.clone().unseal(),td).into())?;


                    // 增加 gas 计数器
                    let mut cumulative_gas = cumulative_gas.lock().unwrap();
                    *cumulative_gas += block.block.gas_used;
                    let mut block_counter = block_counter.lock().unwrap();
                    *block_counter += 1;

                }
                Ok(true)
            }));
        }

        for thread in threads {
            match thread.join() {
                Ok(result) => result?,
                Err(e) => return Err(eyre::eyre!("Thread join error: {:?}", e)),
            };
        }

        Ok(())
    }

    // 获取系统 CPU 核心数
    fn get_cpu_count(&self) -> usize {
        num_cpus::get()
    }
}

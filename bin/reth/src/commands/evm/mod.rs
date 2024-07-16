//! Main evm command for launching a evm tools

use clap::Parser;
use eyre::Result;
use tracing::{info, debug};
use std::sync::{Arc, Mutex};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use reth_blockchain_tree::{BlockchainTree, BlockchainTreeConfig, ShareableBlockchainTree, TreeExternals};
use reth_cli_commands::common::{AccessRights, Environment, EnvironmentArgs};
use reth_consensus::Consensus;
use reth_evm::execute::{BatchExecutor, BlockExecutorProvider};
use reth_primitives::constants::gas_units::format_gas_throughput;
use reth_provider::{BlockReader, ChainSpecProvider, HeaderProvider};
use reth_provider::providers::BlockchainProvider;
use reth_prune::PruneModes;
use reth_revm::database::StateProviderDatabase;
use crate::beacon_consensus::EthBeaconConsensus;
use crate::macros::block_executor;
use crate::providers::{BlockNumReader, ProviderError, StateProviderFactory};

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
    /// step size for loop
    #[arg(long, alias = "step", short = 's', default_value = "10")]
    step_size: usize,
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
            TreeExternals::new(provider_factory.clone(), consensus, executor);
        let tree = BlockchainTree::new(tree_externals, BlockchainTreeConfig::default(), PruneModes::none())?;
        let blockchain_tree = Arc::new(ShareableBlockchainTree::new(tree));
        let blockchain_db = BlockchainProvider::new(provider_factory.clone(), blockchain_tree)?;

        let provider = provider_factory.provider()?;

        let last = provider.last_block_number()?;

        if self.begin_number > self.end_number {
            eyre::bail!("the begin block number is higher than the end block number")
        }
        if  self.end_number > last {
            eyre::bail!("The end block number is higher than the latest block number")
        }


        // 获取 CPU 核心数，减一作为线程数
        let cpu_count = self.get_cpu_count() * 2;
        let thread_count = cpu_count - 1;

        // 计算每个线程处理的区间大小
        let range_per_thread = (self.end_number - self.begin_number + 1) / thread_count as u64;

        let mut threads: Vec<JoinHandle<Result<bool>>> = Vec::with_capacity(thread_count);


        // 创建共享 gas 计数器
        let cumulative_gas = Arc::new(Mutex::new(0));
        let block_counter = Arc::new(Mutex::new(self.begin_number -1));
        let txs_counter = Arc::new(Mutex::new(0));

        // 创建状态输出线程
        {
            let cumulative_gas = Arc::clone(&cumulative_gas);
            let block_counter = Arc::clone(&block_counter);
            let txs_counter = Arc::clone(&txs_counter);
            let start = Instant::now();

            thread::spawn(move || {
                let mut previous_cumulative_gas:u64 = 0;
                let mut previous_block_counter:u64 = self.begin_number -1;
                let mut previous_txs_counter:u64 = 0;
                loop {
                    thread::sleep(Duration::from_secs(1));

                    let current_cumulative_gas =  cumulative_gas.lock().unwrap();
                    let diff_gas = *current_cumulative_gas - previous_cumulative_gas;
                    previous_cumulative_gas = *current_cumulative_gas;

                    let current_block_counter =  block_counter.lock().unwrap();
                    let diff_block = *current_block_counter - previous_block_counter;
                    previous_block_counter = *current_block_counter;

                    let current_txs_counter =  txs_counter.lock().unwrap();
                    let diff_txs = *current_txs_counter - previous_txs_counter;
                    previous_txs_counter = *current_txs_counter;

                    info!(
                        target:"exex::evm",
                        blocks = ?current_block_counter,
                        txs = ?current_txs_counter,
                        BPS = ?diff_block,
                        TPS = ?diff_txs,
                        throughput = format_gas_throughput(diff_gas, Duration::from_secs(1)),
                        time = ?start.elapsed(),
                        "Execution progress"
                     );
                    if *current_block_counter >= (self.end_number - self.begin_number +1) {
                        // return Ok(true);
                    }
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

            let cumulative_gas_clone = Arc::clone(&cumulative_gas);
            let block_counter_clone = Arc::clone(&block_counter);
            let txs_counter_clone = Arc::clone(&txs_counter);

            let provider_factory = provider_factory.clone();
            let blockchain_db = blockchain_db.clone();

            let mut td = blockchain_db.header_td_by_number(thread_start -1)?
                .ok_or_else(|| ProviderError::HeaderNotFound(thread_start.into()))?;

            debug!(
                target:"exex::evm",
                td=?td,
                thread_start=?thread_start,
                "fetch td"
            );
            // let db = StateProviderDatabase::new(blockchain_db.history_by_block_number(thread_start -1)?);
            // let executor = block_executor!(provider_factory.chain_spec());
            // let mut executor = executor.batch_executor(db, PruneModes::none());

            threads.push(thread::spawn(move || {
                let thread_id = thread::current().id();
                for loop_start in (thread_start..=thread_end).step_by(self.step_size) {
                    let loop_end = std::cmp::min(loop_start + self.step_size as u64 - 1, thread_end);
                    let db = StateProviderDatabase::new(blockchain_db.history_by_block_number(loop_start-1)?);
                    let executor = block_executor!(provider_factory.chain_spec());
                    let mut executor = executor.batch_executor(db);

                    let blocks = blockchain_db.block_with_senders_range(loop_start..=loop_end).unwrap();

                    let start = Instant::now();
                    let mut step_cumulative_gas:u64 = 0;
                    let mut step_txs_counter:usize = 0;

                    executor.execute_and_verify_many(blocks.iter()
                        .map(|block| {
                            let result = (block, td).into();
                            td += block.header.difficulty;
                            step_cumulative_gas += block.block.gas_used;
                            step_txs_counter += block.block.body.len();
                            // info!(target:"exex::evm", block_number=block.block.header.number, txs_count= block.block.body.len(), "Adding transactions count");
                            result
                        })
                        .collect::<Vec<_>>())?;


                    // Ensure the locks are correctly used without deadlock
                    {
                        *cumulative_gas_clone.lock().unwrap() +=step_cumulative_gas;
                    }
                    {
                        *block_counter_clone.lock().unwrap()+= blocks.len() as u64;
                    }
                    {
                        *txs_counter_clone.lock().unwrap() += step_txs_counter as u64;
                    }

                    debug!(
                        target:"exex::evm",
                        loop_start=loop_start,
                        loop_end=loop_end,
                        txs=step_txs_counter,
                        blocks = blocks.len(),
                        throughput = format_gas_throughput(step_cumulative_gas, start.elapsed()),
                        time = ?start.elapsed(),
                        thread_id = ?thread_id,
                        total_difficulty = ?td,
                        "loop"
                    );

                    drop(executor);
                }
                Ok(true)
            }));
        }

        for thread in threads {
            match thread.join() {
                Ok(res) => {
                    if let Err(e) = res {
                        return Err(eyre::eyre!("Thread execution error: {:?}", e));
                    }
                },
                Err(e) => return Err(eyre::eyre!("Thread join error: {:?}", e)),
            };
        }
        thread::sleep(Duration::from_secs(1));
        Ok(())
    }

    // 获取系统 CPU 核心数
    fn get_cpu_count(&self) -> usize {
        num_cpus::get()
    }
}

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
    #[arg(long, alias = "step", short = 's', default_value = "100")]
    step_size: usize,
}

struct Task {
    start: u64,
    end: u64,
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


        // 创建任务池
        let mut tasks = Vec::new();
        let mut current_start = self.begin_number;
        while current_start <= self.end_number {
            let current_end = std::cmp::min(current_start + self.step_size as u64 - 1, self.end_number);
            tasks.push(Task {
                start: current_start,
                end: current_end,
            });
            current_start = current_end + 1;
        }


        // 获取 CPU 核心数，减一作为线程数
        let thread_count = self.get_cpu_count() * 2 - 1;
        let mut threads: Vec<JoinHandle<Result<bool>>> = Vec::with_capacity(thread_count);


        // 创建共享 gas 计数器
        let task_queue = Arc::new(Mutex::new(tasks));
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

        for _ in 0..thread_count {

            let task_queue = Arc::clone(&task_queue);
            let cumulative_gas = Arc::clone(&cumulative_gas);
            let block_counter = Arc::clone(&block_counter);
            let txs_counter = Arc::clone(&txs_counter);

            let provider_factory = provider_factory.clone();
            let blockchain_db = blockchain_db.clone();

            threads.push(thread::spawn(move || {
                let thread_id = thread::current().id();
                loop  {
                    let task = {
                        let mut queue = task_queue.lock().unwrap();
                        if queue.is_empty() {
                            break;
                        }
                        queue.remove(0)
                    };

                    let mut td = blockchain_db.header_td_by_number(task.start - 1)?
                        .ok_or_else(|| ProviderError::HeaderNotFound(task.start.into()))?;

                    debug!(
                        target: "exex::evm",
                        td = ?td,
                        thread_start = ?task.start,
                        "fetch td"
                    );


                    let db = StateProviderDatabase::new(blockchain_db.history_by_block_number(task.start-1)?);
                    let executor = block_executor!(provider_factory.chain_spec());
                    let mut executor = executor.batch_executor(db);
                    let blocks = blockchain_db.block_with_senders_range(task.start..=task.end).unwrap();

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
                        *cumulative_gas.lock().unwrap() +=step_cumulative_gas;
                    }
                    {
                        *block_counter.lock().unwrap()+= blocks.len() as u64;
                    }
                    {
                        *txs_counter.lock().unwrap() += step_txs_counter as u64;
                    }

                    debug!(
                        target:"exex::evm",
                        task_start = task.start,
                        task_end = task.end,
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

use alloy_consensus::TxReceipt;
use alloy_network::{Network, ReceiptResponse};
use alloy_provider::{DynProvider, Provider, ProviderBuilder};
use eyre::Context;
use reth_chainspec::{mainnet::MAINNET_SPURIOUS_DRAGON_BLOCK, MAINNET};
use reth_consensus::{ConsensusError, FullConsensus};
use reth_ethereum::{
    evm::revm::database::StateProviderDatabase,
    node::{
        api::{NodeTypes, NodeTypesWithDB},
        EthereumNode,
    },
    provider::{
        db::open_db_read_only,
        providers::{ProviderNodeTypes, StaticFileProvider},
        BlockNumReader, ProviderFactory,
    },
    storage::{BlockReader, ReceiptProvider},
    tasks::TaskExecutor,
};
use reth_evm::{execute::Executor, ConfigureEvm};
use reth_primitives_traits::{
    format_gas_throughput, AlloyBlockHeader, Block, BlockBody, BlockTy, GotExpected,
    SignedTransaction,
};
use revm_database::{AlloyDB, WrapDatabaseAsync, WrapDatabaseRef};
use std::{
    future::IntoFuture,
    path::PathBuf,
    sync::{mpsc, Arc},
    time::{Duration, Instant},
};
use tokio::{runtime::Handle, task::JoinSet};
use tracing::{error, info};

pub async fn run<E, N>(
    evm_config: E,
    consensus: Arc<dyn FullConsensus<E::Primitives, Error = ConsensusError>>,
    provider_factory: ProviderFactory<N>,
    min_block: Option<u64>,
    max_block: Option<u64>,
) -> eyre::Result<()>
where
    E: ConfigureEvm + 'static,
    N: ProviderNodeTypes<Primitives = E::Primitives>,
{
    let mut tasks = JoinSet::new();
    let latest_block = provider_factory.best_block_number().unwrap();
    info!(?latest_block, "Starting execution threads");

    let min_block = min_block.unwrap_or(1);
    let max_block = max_block.unwrap_or(latest_block);

    let num_threads = 10;

    let blocks_per_thread = (max_block - min_block) / num_threads;

    let db_at = {
        let provider = provider_factory.clone();
        move |block_number: u64| {
            StateProviderDatabase(provider.history_by_block_number(block_number).unwrap())
        }
    };

    let (stats_tx, stats_rx) = mpsc::channel();
    tasks.spawn_blocking(move || {
        loop {
            std::thread::sleep(Duration::from_secs(10));
            let mut total_gas = 0;
            let mut total_elapsed = 0.0;
            let mut total_blocks = 0;
            while let Ok((gas_used, elapsed)) = stats_rx.try_recv() {
                total_gas += gas_used;
                total_elapsed += elapsed;
                total_blocks += 1;
            }
            info!(gas_per_second=?format_gas_throughput(total_gas, Duration::from_secs_f64(total_elapsed)), "Executed {total_blocks} blocks");
        }
    });

    for i in 0..num_threads {
        let start_block = min_block + i * blocks_per_thread;
        let end_block = start_block + blocks_per_thread;

        let (blocks_tx, blocks_rx) = mpsc::channel();

        // Spawn thread streaming blocks
        {
            let provider = provider_factory.clone();
            tasks.spawn_blocking(move || {
                for block_number in start_block..end_block {
                    let block = provider
                        .block_by_number(block_number.into())?
                        .unwrap()
                        .seal_slow()
                        .try_recover()?;
                    let _ = blocks_tx.send(block);
                }

                eyre::Ok(())
            });
        }

        // Spawn thread executing blocks
        {
            let provider = provider_factory.clone();
            let evm_config = evm_config.clone();
            let consensus = consensus.clone();
            let db_at = db_at.clone();
            let stats_tx = stats_tx.clone();
            tasks.spawn_blocking(move || {
                let mut executor = evm_config.batch_executor(db_at(start_block - 1));
                while let Ok(block) = blocks_rx.recv() {
                    let instant = Instant::now();
                    let result = executor.execute_one(&block)?;
                    if let Err(err) = consensus
                        .validate_block_post_execution(&block, &result)
                        .wrap_err_with(|| format!("Failed to validate block {}", block.number()))
                    {
                        let correct_receipts = provider.receipts_by_block(block.number().into())?.unwrap();

                        for (i, (receipt, correct_receipt)) in
                            result.receipts.iter().zip(correct_receipts.iter()).enumerate()
                        {
                            let expected_gas_used = correct_receipt.cumulative_gas_used() - if i == 0 { 0 } else { correct_receipts[i - 1].cumulative_gas_used() };
                            let got_gas_used = receipt.cumulative_gas_used() - if i == 0 { 0 } else { result.receipts[i - 1].cumulative_gas_used() };
                            if got_gas_used != expected_gas_used {
                                let mismatch = GotExpected {
                                    expected: expected_gas_used,
                                    got: got_gas_used,
                                };
                                let tx_hash = block.body().transactions()[i].tx_hash();
                                error!(number=?block.number(), index=i, ?tx_hash, ?mismatch,"Gas usage mismatch");
                                return Err(err);
                            }
                        }

                        return Err(err);
                    }
                    stats_tx.send((block.gas_used(), instant.elapsed().as_secs_f64())).unwrap();

                    // Reset DB once in a while to avoid OOM
                    if executor.size_hint() > 1_000_000 {
                        executor = evm_config.batch_executor(db_at(block.number()));
                    }
                }

                eyre::Ok(())
            });
        }
    }

    while let Some(result) = tasks.join_next().await {
        if matches!(result, Err(_) | Ok(Err(_))) {
            error!(?result);
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let chain_spec = MAINNET.clone();
    let datadir = PathBuf::from(std::env::var("RETH_DATADIR").unwrap());

    let provider_factory = EthereumNode::provider_factory_builder()
        .db(Arc::new(open_db_read_only(datadir.join("db"), Default::default()).unwrap()))
        .chainspec(chain_spec.clone())
        .static_file(StaticFileProvider::read_only(datadir.join("static_files"), false).unwrap())
        .build_provider_factory();

    run(
        reth_evm_ethereum::EthEvmConfig::new(chain_spec.clone()),
        Arc::new(reth_ethereum_consensus::EthBeaconConsensus::new(chain_spec)),
        provider_factory,
        Some(MAINNET_SPURIOUS_DRAGON_BLOCK),
        None,
    )
    .await?;

    Ok(())
}

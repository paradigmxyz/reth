use alloy_consensus::TxReceipt;
use alloy_network::{Network, ReceiptResponse};
use alloy_provider::{DynProvider, Provider, ProviderBuilder};
use eyre::Context;
use reth_chainspec::{mainnet::MAINNET_SPURIOUS_DRAGON_BLOCK, MAINNET};
use reth_consensus::{ConsensusError, FullConsensus};
use reth_evm::{execute::Executor, ConfigureEvm};
use reth_primitives_traits::{
    format_gas_throughput, AlloyBlockHeader, Block, BlockBody, BlockTy, GotExpected,
    SignedTransaction,
};
use revm_database::{AlloyDB, WrapDatabaseAsync, WrapDatabaseRef};
use std::{
    future::IntoFuture,
    sync::{mpsc, Arc},
    time::{Duration, Instant},
};
use tokio::{runtime::Handle, task::JoinSet};
use tracing::{error, info};

pub async fn run<E, N>(
    evm_config: E,
    consensus: Arc<dyn FullConsensus<E::Primitives, Error = ConsensusError>>,
    provider: DynProvider<N>,
    convert_block: impl Fn(N::BlockResponse) -> BlockTy<E::Primitives> + Send + Copy + 'static,
    min_block: Option<u64>,
    max_block: Option<u64>,
) -> eyre::Result<()>
where
    E: ConfigureEvm + 'static,
    N: Network,
{
    let latest_block = provider.get_block_number().await?;
    info!(?latest_block, "Starting execution threads");

    let min_block = min_block.unwrap_or(1);
    let max_block = max_block.unwrap_or(latest_block);

    let num_threads = 10;

    let blocks_per_thread = (max_block - min_block) / num_threads;

    let mut tasks = JoinSet::new();

    let db_at = {
        let provider = provider.clone();
        move |block_number: u64| {
            WrapDatabaseRef(
                WrapDatabaseAsync::new(AlloyDB::new(provider.clone(), block_number.into()))
                    .unwrap(),
            )
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
            let provider = provider.clone();
            tasks.spawn(async move {
                for block_number in start_block..end_block {
                    let block = provider.get_block(block_number.into()).full().await?.unwrap();
                    let block = convert_block(block).seal_slow().try_recover()?;
                    let _ = blocks_tx.send(block);
                }

                eyre::Ok(())
            });
        }

        // Spawn thread executing blocks
        {
            let provider = provider.clone();
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
                        let correct_receipts = Handle::current()
                            .block_on(provider.get_block_receipts(block.number().into()))?
                            .unwrap();

                        for (i, (receipt, correct_receipt)) in
                            result.receipts.iter().zip(correct_receipts.iter()).enumerate()
                        {
                            let got_gas_used = receipt.cumulative_gas_used() - if i == 0 { 0 } else { result.receipts[i - 1].cumulative_gas_used() };
                            if got_gas_used != correct_receipt.gas_used() {
                                let mismatch = GotExpected {
                                    expected: correct_receipt.gas_used(),
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
    let provider = DynProvider::new(
        ProviderBuilder::new().connect("https://reth-ethereum.ithaca.xyz/rpc").await?,
    );
    run(
        reth_evm_ethereum::EthEvmConfig::new(chain_spec.clone()),
        Arc::new(reth_ethereum_consensus::EthBeaconConsensus::new(chain_spec)),
        provider,
        |block| block.into_consensus().convert_transactions(),
        Some(MAINNET_SPURIOUS_DRAGON_BLOCK),
        None,
    )
    .await?;

    Ok(())
}

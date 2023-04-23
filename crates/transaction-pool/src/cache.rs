//! Support for populating fee history cache when new block is added to the chain

use futures_util::{Stream, StreamExt};
use reth_primitives::{InvalidTransactionError, SealedBlockWithSenders, U256};
use reth_provider::CanonStateNotification;
use reth_rpc_types::{FeeHistoryCache, FeeHistoryCacheItem, TxGasAndReward};
use tracing::warn;

/// Update fee history cache on new block
async fn add_cache_for_block(
    block: &SealedBlockWithSenders,
    fee_history_cache: &FeeHistoryCache,
    reward_percentiles: &Option<Vec<f64>>,
) -> Result<(), InvalidTransactionError> {
    let header = &block.header.header;
    let txs = &block.body;
    let base_fee_per_gas: U256 = header.base_fee_per_gas.unwrap_or_default().try_into().unwrap();

    let gas_used_ratio = header.gas_used as f64 / header.gas_limit as f64;

    let mut sorter = Vec::with_capacity(txs.len());

    for transaction in txs.iter() {
        let reward = transaction
            .effective_gas_price(header.base_fee_per_gas)
            .ok_or(InvalidTransactionError::FeeCapTooLow)?;

        sorter.push(TxGasAndReward { gas_used: header.gas_used as u128, reward });
    }

    sorter.sort();
    let reward_percentiles = reward_percentiles.as_ref().unwrap();
    let mut rewards = Vec::with_capacity(reward_percentiles.len());

    let mut sum_gas_used = sorter[0].gas_used;
    let mut tx_index = 0;

    for percentile in reward_percentiles.iter() {
        let threshhold_gas_used = (header.gas_used as f64) * percentile / 100_f64;

        while sum_gas_used < threshhold_gas_used as u128 && tx_index < txs.len() {
            sum_gas_used += sorter[tx_index].gas_used;
            tx_index += 1;
        }
        rewards.push(U256::from(sorter[tx_index].reward));
    }

    let fee_history_cache_item =
        FeeHistoryCacheItem { hash: None, base_fee_per_gas, gas_used_ratio, reward: Some(rewards) };

    let mut fee_history_cache = fee_history_cache.0.lock().await;

    fee_history_cache.push(header.number, fee_history_cache_item);
    Ok(())
}

/// Populate the fee history cache when new blocks are added to the chain
///
/// This listens for any new blocks and updates the fee history cache
pub async fn populate_cache_on_new_block_addition<St>(
    fee_history_cache: &FeeHistoryCache,
    reward_percentiles: Option<Vec<f64>>,
    mut events: St,
) where
    St: Stream<Item = CanonStateNotification> + Unpin + 'static,
{
    while let Some(event) = events.next().await {
        match event {
            CanonStateNotification::Reorg { old, new } => {
                // let (new_blocks, new_state) = new.inner();
            }
            CanonStateNotification::Revert { old } => {}
            CanonStateNotification::Commit { new } => {
                let (new_blocks, _) = new.inner();
                match add_cache_for_block(&new_blocks.tip(), fee_history_cache, &reward_percentiles)
                    .await
                {
                    Ok(_) => {}
                    Err(e) => {
                        warn!("Failed to add cache for block {:?}: {:?}", new_blocks.tip().hash, e);
                    }
                }
            }
        }
    }
}

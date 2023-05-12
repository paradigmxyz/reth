//! Contains RPC handler implementations for fee history.

use crate::{
    eth::error::{EthApiError, EthResult, InvalidTransactionError},
    EthApi,
};
use reth_network_api::NetworkInfo;
use reth_primitives::{BlockId, BlockNumberOrTag, U256};
use reth_provider::{BlockProviderIdExt, EvmEnvProvider, StateProviderFactory};
use reth_rpc_types::{FeeHistory, FeeHistoryCacheItem, TxGasAndReward};
use reth_transaction_pool::TransactionPool;
use std::collections::BTreeMap;

impl<Client, Pool, Network> EthApi<Client, Pool, Network>
where
    Pool: TransactionPool + Clone + 'static,
    Client: BlockProviderIdExt + StateProviderFactory + EvmEnvProvider + 'static,
    Network: NetworkInfo + Send + Sync + 'static,
{
    /// Returns a suggestion for a gas price for legacy transactions.
    ///
    /// See also: <https://github.com/ethereum/pm/issues/328#issuecomment-853234014>
    pub(crate) async fn gas_price(&self) -> EthResult<U256> {
        let header = self.block(BlockNumberOrTag::Latest);
        let suggested_tip = self.suggested_priority_fee();
        let (header, suggested_tip) = futures::try_join!(header, suggested_tip)?;
        let base_fee = header.and_then(|h| h.base_fee_per_gas).unwrap_or_default();
        Ok(suggested_tip + U256::from(base_fee))
    }

    /// Returns a suggestion for the priority fee (the tip)
    pub(crate) async fn suggested_priority_fee(&self) -> EthResult<U256> {
        // TODO: properly implement sampling https://github.com/ethereum/pm/issues/328#issuecomment-853234014
        Ok(U256::from(1e9 as u64))
    }

    /// Reports the fee history, for the given amount of blocks, up until the newest block
    /// provided.
    pub(crate) async fn fee_history(
        &self,
        block_count: u64,
        newest_block: BlockId,
        reward_percentiles: Option<Vec<f64>>,
    ) -> EthResult<FeeHistory> {
        if block_count == 0 {
            return Ok(FeeHistory::default())
        }

        let Some(previous_to_end_block) = self.inner.client.block_number_for_id(newest_block)? else { return Err(EthApiError::UnknownBlockNumber)};
        let end_block = previous_to_end_block + 1;

        if end_block < block_count {
            return Err(EthApiError::InvalidBlockRange)
        }

        let mut start_block = end_block - block_count;

        if block_count == 1 {
            start_block = previous_to_end_block;
        }

        // if not provided the percentiles are []
        let reward_percentiles = reward_percentiles.unwrap_or_default();

        // checks for rewardPercentile's sorted-ness
        // check if any of rewardPercentile is greater than 100
        // pre 1559 blocks, return 0 for baseFeePerGas
        for window in reward_percentiles.windows(2) {
            if window[0] >= window[1] {
                return Err(EthApiError::InvalidRewardPercentile(window[1]))
            }

            if window[0] < 0.0 || window[0] > 100.0 {
                return Err(EthApiError::InvalidRewardPercentile(window[0]))
            }
        }

        let mut fee_history_cache = self.fee_history_cache.0.lock().await;

        // Sorted map that's populated in two rounds:
        // 1. Cache entries until first non-cached block
        // 2. Database query from the first non-cached block
        let mut fee_history_cache_items = BTreeMap::new();

        let mut first_non_cached_block = None;
        let mut last_non_cached_block = None;
        for block in start_block..=end_block {
            // Check if block exists in cache, and move it to the head of the list if so
            if let Some(fee_history_cache_item) = fee_history_cache.get(&block) {
                fee_history_cache_items.insert(block, fee_history_cache_item.clone());
            } else {
                // If block doesn't exist in cache, set it as a first non-cached block to query it
                // from the database
                first_non_cached_block.get_or_insert(block);
                // And last non-cached block, so we could query the database until we reach it
                last_non_cached_block = Some(block);
            }
        }

        // If we had any cache misses, query the database starting with the first non-cached block
        // and ending with the last
        if let (Some(start_block), Some(end_block)) =
            (first_non_cached_block, last_non_cached_block)
        {
            let header_range = start_block..=end_block;

            let headers = self.inner.client.headers_range(header_range.clone())?;
            let transactions_by_block =
                self.inner.client.transactions_by_block_range(header_range)?;

            let header_tx = headers.iter().zip(&transactions_by_block);

            // We should receive exactly the amount of blocks missing from the cache
            if headers.len() != (end_block - start_block + 1) as usize {
                return Err(EthApiError::InvalidBlockRange)
            }

            // We should receive exactly the amount of blocks missing from the cache
            if transactions_by_block.len() != (end_block - start_block + 1) as usize {
                return Err(EthApiError::InvalidBlockRange)
            }

            for (header, transactions) in header_tx {
                let base_fee_per_gas: U256 = header.base_fee_per_gas.
                        unwrap_or_default(). // Zero for pre-EIP-1559 blocks
                        try_into().unwrap(); // u64 -> U256 won't fail
                let gas_used_ratio = header.gas_used as f64 / header.gas_limit as f64;

                let mut sorter = Vec::with_capacity(transactions.len());
                for transaction in transactions.iter() {
                    let reward = transaction
                        .effective_gas_tip(header.base_fee_per_gas)
                        .ok_or(InvalidTransactionError::FeeCapTooLow)?;

                    sorter.push(TxGasAndReward { gas_used: header.gas_used as u128, reward })
                }

                sorter.sort();

                let mut rewards = Vec::with_capacity(reward_percentiles.len());
                let mut sum_gas_used = sorter[0].gas_used;
                let mut tx_index = 0;

                for percentile in reward_percentiles.iter() {
                    let threshold_gas_used = (header.gas_used as f64) * percentile / 100_f64;
                    while sum_gas_used < threshold_gas_used as u128 && tx_index < transactions.len()
                    {
                        tx_index += 1;
                        sum_gas_used += sorter[tx_index].gas_used;
                    }

                    rewards.push(U256::from(sorter[tx_index].reward));
                }

                let fee_history_cache_item = FeeHistoryCacheItem {
                    hash: None,
                    base_fee_per_gas,
                    gas_used_ratio,
                    reward: Some(rewards),
                };

                // Insert missing cache entries in the map for further response composition from
                // it
                fee_history_cache_items.insert(header.number, fee_history_cache_item.clone());
                // And populate the cache with new entries
                fee_history_cache.push(header.number, fee_history_cache_item);
            }
        }

        // get the first block in the range from the db
        let oldest_block_hash =
            self.inner.client.block_hash(start_block)?.ok_or(EthApiError::UnknownBlockNumber)?;

        // Set the hash in cache items if the block is present in the cache
        if let Some(cache_item) = fee_history_cache_items.get_mut(&start_block) {
            cache_item.hash = Some(oldest_block_hash);
        }

        if let Some(cache_item) = fee_history_cache.get_mut(&start_block) {
            cache_item.hash = Some(oldest_block_hash);
        }

        // `fee_history_cache_items` now contains full requested block range (populated from both
        // cache and database), so we can iterate over it in order and populate the response fields
        let base_fee_per_gas =
            fee_history_cache_items.values().map(|item| item.base_fee_per_gas).collect();

        let mut gas_used_ratio: Vec<f64> =
            fee_history_cache_items.values().map(|item| item.gas_used_ratio).collect();

        let mut rewards: Vec<Vec<_>> =
            fee_history_cache_items.values().filter_map(|item| item.reward.clone()).collect();

        // gasUsedRatio doesn't have data for next block in this case the last block
        gas_used_ratio.pop();
        rewards.pop();

        Ok(FeeHistory {
            base_fee_per_gas,
            gas_used_ratio,
            oldest_block: U256::from(start_block),
            reward: Some(rewards),
        })
    }
}

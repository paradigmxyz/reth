//! Contains RPC handler implementations for fee history.

use crate::{
    eth::error::{EthApiError, EthResult},
    EthApi,
};
use reth_network_api::NetworkInfo;
use reth_primitives::{BlockId, Header, U256, U64};
use reth_provider::{BlockProvider, EvmEnvProvider, StateProviderFactory};
use reth_rpc_types::{FeeHistory, FeeHistoryCacheItem, TxGasAndReward};
use reth_transaction_pool::TransactionPool;
use std::collections::BTreeMap;

impl<Client, Pool, Network> EthApi<Client, Pool, Network>
where
    Pool: TransactionPool + Clone + 'static,
    Client: BlockProvider + StateProviderFactory + EvmEnvProvider + 'static,
    Network: NetworkInfo + Send + Sync + 'static,
{
    /// Reports the fee history, for the given amount of blocks, up until the newest block
    /// provided.
    pub(crate) async fn fee_history(
        &self,
        block_count: U64,
        newest_block: BlockId,
        reward_percentiles: Option<Vec<f64>>,
    ) -> EthResult<FeeHistory> {
        let block_count = block_count.as_u64();

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

        let reward_percentiles_unwrapped = reward_percentiles.unwrap_or(vec![]);

        // checks for rewardPercentile's sorted-ness
        // check if any of rewardPercentile is greater than 100
        // pre 1559 blocks, return 0 for baseFeePerGas
        for i in 1..reward_percentiles_unwrapped.len() {
            if reward_percentiles_unwrapped[i] <= reward_percentiles_unwrapped[i - 1] {
                return Err(EthApiError::InvalidRewardPercentile(reward_percentiles_unwrapped[i]))
            }

            if reward_percentiles_unwrapped[i] < 0.0 && reward_percentiles_unwrapped[i] > 100.0 {
                return Err(EthApiError::InvalidRewardPercentile(reward_percentiles_unwrapped[i]))
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
            let headers: Vec<Header> =
                self.inner.client.headers_range(start_block..=end_block + 1)?;

            let transactions =
                self.inner.client.transactions_by_block_range(start_block..=end_block + 1)?;

            // We should receive exactly the amount of blocks missing from the cache
            if headers.len() != (end_block - start_block + 1) as usize {
                return Err(EthApiError::InvalidBlockRange)
            }

            // We should receive exactly the amount of blocks missing from the cache
            if transactions.len() != (end_block - start_block + 1) as usize {
                return Err(EthApiError::InvalidBlockRange)
            }

            for index in 0..(end_block - start_block + 1) as usize {
                let option_header = headers.get(index);
                let option_transaction = transactions.get(index);

                if let Some(header) = option_header {
                    let base_fee_per_gas: U256 = header.base_fee_per_gas.
                            unwrap_or_default(). // Zero for pre-EIP-1559 blocks
                            try_into().unwrap(); // u64 -> U256 won't fail
                    let gas_used_ratio = header.gas_used as f64 / header.gas_limit as f64;

                    let mut reward: Vec<U256> = vec![];

                    if let Some(transactions) = option_transaction {
                        let mut sorter: Vec<TxGasAndReward> = vec![];
                        for transaction in transactions {
                            // let max_priority_fee_per_gas =
                            // transaction.max_priority_fee_per_gas().unwrap_or(self.gas_price().
                            // await.unwrap().try_into().unwrap()); // todo: unwrap_or => gas_price
                            // or 1??
                            let max_priority_fee_per_gas =
                                transaction.max_priority_fee_per_gas().unwrap_or(1);
                            let max_fee_per_gas = transaction.max_fee_per_gas();
                            let base_fee = header.base_fee_per_gas.unwrap_or_default();

                            if max_priority_fee_per_gas < base_fee as u128 {
                                return Err(EthApiError::ConflictingFeeFieldsInRequest)
                            }

                            // todo: get gas fees for tx and also gas price
                            sorter.push(TxGasAndReward {
                                gas_used: header.gas_used as u128,
                                reward: std::cmp::min(
                                    max_priority_fee_per_gas,
                                    max_fee_per_gas - base_fee as u128,
                                ),
                            })
                        }

                        sorter.sort();

                        let mut sum_gas_used = sorter[0].gas_used;
                        let mut tx_index = 0;

                        for i in &reward_percentiles_unwrapped {
                            let threshold_gas_used = (header.gas_used as f64) * i / 100_f64;
                            while sum_gas_used < threshold_gas_used as u128 &&
                                tx_index < transactions.len()
                            {
                                tx_index += 1;
                                sum_gas_used += sorter[tx_index].reward;
                            }

                            reward.push(U256::from(sorter[tx_index].reward));
                        }
                    }

                    let fee_history_cache_item = FeeHistoryCacheItem {
                        hash: None,
                        base_fee_per_gas,
                        gas_used_ratio,
                        reward: Some(reward), // TODO: calculate rewards per transaction
                    };

                    // Insert missing cache entries in the map for further response composition from
                    // it
                    fee_history_cache_items.insert(header.number, fee_history_cache_item.clone());
                    // And populate the cache with new entries
                    fee_history_cache.push(header.number, fee_history_cache_item);
                }
            }
        }

        let oldest_block_hash = self.inner.client.block_hash(start_block)?.unwrap();

        fee_history_cache_items.get_mut(&start_block).unwrap().hash = Some(oldest_block_hash);
        fee_history_cache.get_mut(&start_block).unwrap().hash = Some(oldest_block_hash);

        let base_fee_per_gas =
            fee_history_cache_items.values().map(|item| item.base_fee_per_gas).collect();

        let mut gas_used_ratio: Vec<f64> =
            fee_history_cache_items.values().map(|item| item.gas_used_ratio).collect();

        let rewards: Option<Vec<Vec<U256>>> =
            fee_history_cache_items.values().map(|item| item.reward.clone()).collect();

        let mut rewards = rewards.unwrap();
        // gasUsedRatio doesn't has data for next block in this case the last block
        gas_used_ratio.pop();
        rewards.pop();

        // `fee_history_cache_items` now contains full requested block range (populated from both
        // cache and database), so we can iterate over it in order and populate the response fields
        Ok(FeeHistory {
            base_fee_per_gas,
            gas_used_ratio,
            // oldest_block: U256::from_be_bytes(oldest_block_hash.0),
            oldest_block: U256::from(start_block),
            reward: Some(rewards),
        })
    }
}

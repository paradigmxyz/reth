//! Contains RPC handler implementations for fee history.

use crate::{
    eth::error::{EthApiError, EthResult},
    EthApi,
};
use reth_network_api::NetworkInfo;
use reth_primitives::{basefee::calculate_next_block_base_fee, BlockId, BlockNumberOrTag, U256};
use reth_provider::{BlockReaderIdExt, EvmEnvProvider, StateProviderFactory};
use reth_rpc_types::{FeeHistory, TxGasAndReward};
use reth_transaction_pool::TransactionPool;

impl<Provider, Pool, Network> EthApi<Provider, Pool, Network>
where
    Pool: TransactionPool + Clone + 'static,
    Provider: BlockReaderIdExt + StateProviderFactory + EvmEnvProvider + 'static,
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
        self.gas_oracle().suggest_tip_cap().await
    }

    /// Reports the fee history, for the given amount of blocks, up until the newest block
    /// provided.
    pub(crate) async fn fee_history(
        &self,
        block_count: u64,
        newest_block: BlockNumberOrTag,
        reward_percentiles: Option<Vec<f64>>,
    ) -> EthResult<FeeHistory> {
        if block_count == 0 {
            return Ok(FeeHistory::default())
        }

        // The spec states that you can request a maximum of 1024 blocks.
        if block_count > 1024 {
            return Err(EthApiError::InvalidBlockRange)
        }

        let Some(end_block) = self.inner.provider.block_number_for_id(newest_block.into())? else {
            return Err(EthApiError::UnknownBlockNumber) };

        // Check that we would not be querying outside of genesis
        if end_block < block_count {
            return Err(EthApiError::InvalidBlockRange)
        }

        // If reward percentiles were specified, we need to validate that they are monotonically
        // increasing and 0 <= p <= 100
        //
        // Note: The types used ensure that the percentiles are never < 0
        if let Some(percentiles) = &reward_percentiles {
            if percentiles.windows(2).any(|w| w[0] > w[1] || w[0] > 100.) {
                return Err(EthApiError::InvalidRewardPercentiles)
            }
        }

        // Fetch the headers and ensure we got all of them
        //
        // Treat a request for 1 block as a request for `newest_block..=newest_block`,
        // otherwise `newest_block - 2
        let start_block = end_block - block_count + 1;
        let headers = self.inner.provider.headers_range(start_block..=end_block)?;
        if headers.len() != block_count as usize {
            return Err(EthApiError::InvalidBlockRange)
        }

        // Collect base fees, gas usage ratios and (optionally) reward percentile data
        let mut base_fee_per_gas: Vec<U256> = Vec::new();
        let mut gas_used_ratio: Vec<f64> = Vec::new();
        let mut rewards: Vec<Vec<U256>> = Vec::new();
        for header in &headers {
            let header_base_fee_per_gas = header.base_fee_per_gas.unwrap_or_default();
            base_fee_per_gas.push(U256::try_from(header_base_fee_per_gas).unwrap());
            gas_used_ratio.push(header.gas_used as f64 / header.gas_limit as f64);

            // Percentiles were specified, so we need to collect reward percentile ino
            if let Some(percentiles) = &reward_percentiles {
                let Some(receipts) =
                    self.inner.provider.receipts_by_block(header.number.into())? else {
                    // If there are no receipts, then we do not have all info on the block
                    return Err(EthApiError::InvalidBlockRange)
                };
                let Some(mut transactions): Option<Vec<_>> = self
                    .inner
                    .provider
                    .transactions_by_block(header.number.into())?
                    .map(|txs|txs
                    .into_iter()
                    .zip(receipts.into_iter())
                    .map(|(tx, receipt)| TxGasAndReward {
                        gas_used: header.gas_used - receipt.cumulative_gas_used,
                        reward: tx.effective_gas_tip(header.base_fee_per_gas).unwrap_or_default(),
                    })
                    .collect()) else {
                        // If there are no transactions, then we do not have all info on the block
                        return Err(EthApiError::InvalidBlockRange)
                    };

                // Sort the transactions by their rewards in ascending order
                transactions.sort_by_key(|tx| tx.reward);

                println!("txs: {}", transactions.len());

                // Find the transaction that corresponds to the given percentile
                //
                // We use a `tx_index` here that is shared across all percentiles, since we know
                // the percentiles are monotonically increasing.
                let mut tx_index = 0;
                let mut cumulative_gas_used =
                    transactions.first().map(|tx| tx.gas_used).unwrap_or_default();
                let mut rewards_in_block = Vec::new();
                for percentile in percentiles {
                    // Empty blocks should return in a zero row
                    if transactions.is_empty() {
                        rewards_in_block.push(U256::ZERO);
                        continue
                    }

                    let threshold = (header.gas_used as f64 * percentile / 100.) as u64;
                    println!(
                        "percentile: {}, cumulative gas: {}, threshold: {}",
                        percentile, cumulative_gas_used, threshold
                    );
                    while cumulative_gas_used < threshold && tx_index < transactions.len() - 1 {
                        tx_index += 1;
                        cumulative_gas_used += transactions[tx_index].gas_used;
                        println!(
                            "percentile: {}, cumulative gas: {}, threshold: {}, tx_index: {}",
                            percentile, cumulative_gas_used, threshold, tx_index
                        );
                    }
                    println!("found tx: {}", tx_index);
                    rewards_in_block.push(U256::from(transactions[tx_index].reward));
                }
                rewards.push(rewards_in_block);
            }
        }

        // The spec states that `base_fee_per_gas` "[..] includes the next block after the newest of
        // the returned range, because this value can be derived from the newest block"
        //
        // The unwrap is safe since we checked earlier that we got at least 1 header.
        let last_header = headers.last().unwrap();
        base_fee_per_gas.push(U256::from(calculate_next_block_base_fee(
            last_header.gas_used,
            last_header.gas_limit,
            last_header.base_fee_per_gas.unwrap_or_default(),
        )));

        Ok(FeeHistory {
            base_fee_per_gas,
            gas_used_ratio,
            oldest_block: U256::from(start_block),
            reward: reward_percentiles.map(|_| rewards),
        })
    }
}

//! Contains RPC handler implementations for fee history.

use crate::{
    eth::{
        api::fee_history::{calculate_reward_percentiles_for_block, FeeHistoryEntry},
        error::{EthApiError, EthResult},
    },
    EthApi,
};
use reth_network_api::NetworkInfo;
use reth_primitives::{basefee::calculate_next_block_base_fee, BlockNumberOrTag, U256};
use reth_provider::{BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, StateProviderFactory};
use reth_rpc_types::FeeHistory;
use reth_transaction_pool::TransactionPool;
use tracing::debug;

impl<Provider, Pool, Network> EthApi<Provider, Pool, Network>
where
    Pool: TransactionPool + Clone + 'static,
    Provider:
        BlockReaderIdExt + ChainSpecProvider + StateProviderFactory + EvmEnvProvider + 'static,
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

    /// Returns a suggestion for a gas price for blob transactions.
    pub(crate) async fn blob_gas_price(&self) -> EthResult<U256> {
        self.block(BlockNumberOrTag::Latest)
            .await?
            .and_then(|h| h.next_block_blob_fee())
            .ok_or(EthApiError::ExcessBlobGasNotSet)
            .map(U256::from)
    }

    /// Returns a suggestion for the priority fee (the tip)
    pub(crate) async fn suggested_priority_fee(&self) -> EthResult<U256> {
        self.gas_oracle().suggest_tip_cap().await
    }

    /// Reports the fee history, for the given amount of blocks, up until the given newest block.
    ///
    /// If `reward_percentiles` are provided the [FeeHistory] will include the _approximated_
    /// rewards for the requested range.
    pub(crate) async fn fee_history(
        &self,
        mut block_count: u64,
        newest_block: BlockNumberOrTag,
        reward_percentiles: Option<Vec<f64>>,
    ) -> EthResult<FeeHistory> {
        if block_count == 0 {
            return Ok(FeeHistory::default())
        }

        // See https://github.com/ethereum/go-ethereum/blob/2754b197c935ee63101cbbca2752338246384fec/eth/gasprice/feehistory.go#L218C8-L225
        let max_fee_history = if reward_percentiles.is_none() {
            self.gas_oracle().config().max_header_history
        } else {
            self.gas_oracle().config().max_block_history
        };

        if block_count > max_fee_history {
            debug!(
                requested = block_count,
                truncated = max_fee_history,
                "Sanitizing fee history block count"
            );
            block_count = max_fee_history
        }

        let Some(end_block) = self.provider().block_number_for_id(newest_block.into())? else {
            return Err(EthApiError::UnknownBlockNumber)
        };

        // need to add 1 to the end block to get the correct (inclusive) range
        let end_block_plus = end_block + 1;
        // Ensure that we would not be querying outside of genesis
        if end_block_plus < block_count {
            block_count = end_block_plus;
        }

        // If reward percentiles were specified, we
        // need to validate that they are monotonically
        // increasing and 0 <= p <= 100
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
        // SAFETY: We ensured that block count is capped
        let start_block = end_block_plus - block_count;

        // Collect base fees, gas usage ratios and (optionally) reward percentile data
        let mut base_fee_per_gas: Vec<U256> = Vec::new();
        let mut gas_used_ratio: Vec<f64> = Vec::new();
        let mut rewards: Vec<Vec<U256>> = Vec::new();

        // Check if the requested range is within the cache bounds
        let fee_entries = self.fee_history_cache().get_history(start_block, end_block).await;

        if let Some(fee_entries) = fee_entries {
            if fee_entries.len() != block_count as usize {
                return Err(EthApiError::InvalidBlockRange)
            }

            for entry in &fee_entries {
                base_fee_per_gas.push(U256::from(entry.base_fee_per_gas));
                gas_used_ratio.push(entry.gas_used_ratio);

                if let Some(percentiles) = &reward_percentiles {
                    let mut block_rewards = Vec::with_capacity(percentiles.len());
                    for &percentile in percentiles.iter() {
                        block_rewards.push(self.approximate_percentile(entry, percentile));
                    }
                    rewards.push(block_rewards);
                }
            }
            let last_entry = fee_entries.last().expect("is not empty");

            let last_entry_timestamp = self
                .provider()
                .header_by_hash_or_number(last_entry.header_hash.into())?
                .map(|h| h.timestamp)
                .unwrap_or_default();

            base_fee_per_gas.push(U256::from(calculate_next_block_base_fee(
                last_entry.gas_used,
                last_entry.gas_limit,
                last_entry.base_fee_per_gas,
                self.provider().chain_spec().base_fee_params(last_entry_timestamp),
            )));
        } else {
            // read the requested header range
            let headers = self.provider().sealed_headers_range(start_block..=end_block)?;
            if headers.len() != block_count as usize {
                return Err(EthApiError::InvalidBlockRange)
            }

            for header in &headers {
                base_fee_per_gas.push(U256::from(header.base_fee_per_gas.unwrap_or_default()));
                gas_used_ratio.push(header.gas_used as f64 / header.gas_limit as f64);

                // Percentiles were specified, so we need to collect reward percentile ino
                if let Some(percentiles) = &reward_percentiles {
                    let (transactions, receipts) = self
                        .cache()
                        .get_transactions_and_receipts(header.hash)
                        .await?
                        .ok_or(EthApiError::InvalidBlockRange)?;
                    rewards.push(
                        calculate_reward_percentiles_for_block(
                            percentiles,
                            header.gas_used,
                            header.base_fee_per_gas.unwrap_or_default(),
                            &transactions,
                            &receipts,
                        )
                        .unwrap_or_default(),
                    );
                }
            }

            // The spec states that `base_fee_per_gas` "[..] includes the next block after the
            // newest of the returned range, because this value can be derived from the
            // newest block"
            //
            // The unwrap is safe since we checked earlier that we got at least 1 header.

            // The spec states that `base_fee_per_gas` "[..] includes the next block after the
            // newest of the returned range, because this value can be derived from the
            // newest block"
            //
            // The unwrap is safe since we checked earlier that we got at least 1 header.
            let last_header = headers.last().expect("is present");
            base_fee_per_gas.push(U256::from(calculate_next_block_base_fee(
                last_header.gas_used,
                last_header.gas_limit,
                last_header.base_fee_per_gas.unwrap_or_default(),
                self.provider().chain_spec().base_fee_params(last_header.timestamp),
            )));
        };

        Ok(FeeHistory {
            base_fee_per_gas,
            gas_used_ratio,
            oldest_block: U256::from(start_block),
            reward: reward_percentiles.map(|_| rewards),
        })
    }

    /// Approximates reward at a given percentile for a specific block
    /// Based on the configured resolution
    fn approximate_percentile(&self, entry: &FeeHistoryEntry, requested_percentile: f64) -> U256 {
        let resolution = self.fee_history_cache().resolution();
        let rounded_percentile =
            (requested_percentile * resolution as f64).round() / resolution as f64;
        let clamped_percentile = rounded_percentile.clamp(0.0, 100.0);

        // Calculate the index in the precomputed rewards array
        let index = (clamped_percentile / (1.0 / resolution as f64)).round() as usize;
        // Fetch the reward from the FeeHistoryEntry
        entry.rewards.get(index).cloned().unwrap_or(U256::ZERO)
    }
}

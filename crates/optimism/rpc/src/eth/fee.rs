use super::{OpEthApi, OpNodeCore};
use alloy_consensus::{BlockHeader, Transaction, TxReceipt};
use alloy_eips::{eip7840::BlobParams, BlockNumberOrTag};
use alloy_primitives::Sealable;
use alloy_primitives::U256;
use alloy_rpc_types_eth::FeeHistory;
use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks};
use reth_optimism_consensus::next_block_base_fee as optimism_next_block_base_fee;
use reth_primitives_traits::{Block, BlockBody};
use reth_rpc_eth_api::{
    helpers::{EthFees, LoadBlock, LoadFee},
    FromEthApiError,
};
use reth_rpc_eth_types::{
    fee_history::calculate_reward_percentiles_for_block, EthApiError, FeeHistoryCache,
    GasPriceOracle,
};
use reth_rpc_server_types::constants::gas_oracle::DEFAULT_MIN_SUGGESTED_PRIORITY_FEE;
use reth_storage_api::{
    BlockIdReader, BlockReader, BlockReaderIdExt, HeaderProvider, ReceiptProvider,
    StateProviderFactory,
};
use tracing::debug;

impl<N> LoadFee for OpEthApi<N>
where
    Self: LoadBlock<Provider = N::Provider>,
    N: OpNodeCore<
        Provider: BlockReaderIdExt
                      + ChainSpecProvider<
            ChainSpec: EthChainSpec + EthereumHardforks + reth_optimism_forks::OpHardforks,
        > + StateProviderFactory,
    >,
{
    #[inline]
    fn gas_oracle(&self) -> &GasPriceOracle<Self::Provider> {
        self.inner.eth_api.gas_oracle()
    }

    #[inline]
    fn fee_history_cache(&self) -> &FeeHistoryCache {
        self.inner.eth_api.fee_history_cache()
    }

    /// Optimism-specific priority fee suggestion
    async fn suggested_priority_fee(&self) -> Result<U256, Self::Error>
    where
        Self: 'static,
    {
        // Delegate to the Optimism-specific implementation that mirrors op-geth's SuggestOptimismPriorityFee
        self.suggest_optimism_priority_fee().await
    }
}

impl<N> EthFees for OpEthApi<N>
where
    Self: LoadFee,
    N: OpNodeCore<
        Provider: BlockReaderIdExt
                      + ChainSpecProvider<
            ChainSpec: EthChainSpec + EthereumHardforks + reth_optimism_forks::OpHardforks,
        > + StateProviderFactory,
    >,
{
    /// Reports the fee history, for the given amount of blocks, up until the given newest block.
    ///
    /// If `reward_percentiles` are provided the [`FeeHistory`] will include the _approximated_
    /// rewards for the requested range.
    async fn fee_history(
        &self,
        mut block_count: u64,
        mut newest_block: BlockNumberOrTag,
        reward_percentiles: Option<Vec<f64>>,
    ) -> Result<FeeHistory, Self::Error> {
        if block_count == 0 {
            return Ok(FeeHistory::default());
        }

        // ensure the given reward percentiles aren't excessive
        if reward_percentiles.as_ref().map(|perc| perc.len() as u64)
            > Some(self.gas_oracle().config().max_reward_percentile_count)
        {
            return Err(EthApiError::InvalidRewardPercentiles.into());
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

        if newest_block.is_pending() {
            // cap the target block since we don't have fee history for the pending block
            newest_block = BlockNumberOrTag::Latest;
            // account for missing pending block
            block_count = block_count.saturating_sub(1);
        }

        let end_block = self
            .inner
            .eth_api
            .provider()
            .block_number_for_id(newest_block.into())
            .map_err(Self::Error::from_eth_err)?
            .ok_or(EthApiError::HeaderNotFound(newest_block.into()))?;

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
                return Err(EthApiError::InvalidRewardPercentiles.into());
            }
        }

        // Fetch the headers and ensure we got all of them
        //
        // Treat a request for 1 block as a request for `newest_block..=newest_block`,
        // otherwise `newest_block - 2`
        // NOTE: We ensured that block count is capped
        let start_block = end_block_plus - block_count;

        // Collect base fees, gas usage ratios and (optionally) reward percentile data
        let mut base_fee_per_gas: Vec<u128> = Vec::new();
        let mut gas_used_ratio: Vec<f64> = Vec::new();

        let mut base_fee_per_blob_gas: Vec<u128> = Vec::new();
        let mut blob_gas_used_ratio: Vec<f64> = Vec::new();

        let mut rewards: Vec<Vec<u128>> = Vec::new();

        // Check if the requested range is within the cache bounds
        let fee_entries = self.fee_history_cache().get_history(start_block, end_block).await;

        if let Some(fee_entries) = fee_entries {
            if fee_entries.len() != block_count as usize {
                return Err(EthApiError::InvalidBlockRange.into());
            }

            for entry in &fee_entries {
                base_fee_per_gas.push(entry.base_fee_per_gas as u128);
                gas_used_ratio.push(entry.gas_used_ratio);
                base_fee_per_blob_gas.push(entry.base_fee_per_blob_gas.unwrap_or_default());
                blob_gas_used_ratio.push(entry.blob_gas_used_ratio);

                if let Some(percentiles) = &reward_percentiles {
                    let mut block_rewards = Vec::with_capacity(percentiles.len());
                    for &percentile in percentiles {
                        block_rewards.push(self.approximate_percentile(entry, percentile));
                    }
                    rewards.push(block_rewards);
                }
            }
            let last_entry = fee_entries.last().expect("is not empty");

            // Also need to include the `base_fee_per_gas` and `base_fee_per_blob_gas` for the
            // next block
            // Use Optimism-specific base fee calculation for consistency
            // We need to get the full header from cache to use Optimism-specific calculation
            let last_header = self
                .inner
                .eth_api
                .cache()
                .get_header(last_entry.header_hash)
                .await
                .map_err(Self::Error::from_eth_err)?;

            let next_base_fee = self
                .calculate_optimism_next_block_base_fee(&last_header, last_entry.timestamp)
                .unwrap_or_default() as u128;
            base_fee_per_gas.push(next_base_fee);

            base_fee_per_blob_gas.push(last_entry.next_block_blob_fee().unwrap_or_default());
        } else {
            // read the requested header range
            let headers = self
                .inner
                .eth_api
                .provider()
                .headers_range(start_block..=end_block)
                .map_err(Self::Error::from_eth_err)?;
            if headers.len() != block_count as usize {
                return Err(EthApiError::InvalidBlockRange.into());
            }

            for header in &headers {
                base_fee_per_gas.push(header.base_fee_per_gas().unwrap_or_default() as u128);
                gas_used_ratio.push(header.gas_used() as f64 / header.gas_limit() as f64);

                let blob_params = self
                    .inner
                    .eth_api
                    .provider()
                    .chain_spec()
                    .blob_params_at_timestamp(header.timestamp())
                    .unwrap_or_else(BlobParams::cancun);

                base_fee_per_blob_gas.push(header.blob_fee(blob_params).unwrap_or_default());
                blob_gas_used_ratio.push(
                    header.blob_gas_used().unwrap_or_default() as f64
                        / blob_params.max_blob_gas_per_block() as f64,
                );

                // Percentiles were specified, so we need to collect reward percentile info
                if let Some(percentiles) = &reward_percentiles {
                    let (block, receipts) = self
                        .inner
                        .eth_api
                        .cache()
                        .get_block_and_receipts(header.hash_slow())
                        .await
                        .map_err(Self::Error::from_eth_err)?
                        .ok_or(EthApiError::InvalidBlockRange)?;
                    rewards.push(
                        calculate_reward_percentiles_for_block(
                            percentiles,
                            header.gas_used(),
                            header.base_fee_per_gas().unwrap_or_default(),
                            block.body().transactions(),
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
            let last_header = headers.last().expect("is present");
            let next_base_fee = self
                .calculate_optimism_next_block_base_fee(last_header, last_header.timestamp())
                .unwrap_or_default() as u128;
            base_fee_per_gas.push(next_base_fee);

            // Same goes for the `base_fee_per_blob_gas`:
            // > "[..] includes the next block after the newest of the returned range, because this value can be derived from the newest block.
            base_fee_per_blob_gas.push(
                last_header
                    .maybe_next_block_blob_fee(
                        self.inner
                            .eth_api
                            .provider()
                            .chain_spec()
                            .blob_params_at_timestamp(last_header.timestamp()),
                    )
                    .unwrap_or_default(),
            );
        };

        Ok(FeeHistory {
            base_fee_per_gas,
            gas_used_ratio,
            base_fee_per_blob_gas,
            blob_gas_used_ratio,
            oldest_block: start_block,
            reward: reward_percentiles.map(|_| rewards),
        })
    }
}

impl<N> OpEthApi<N>
where
    N: OpNodeCore<
        Provider: BlockReaderIdExt
                      + ChainSpecProvider<
            ChainSpec: EthChainSpec + EthereumHardforks + reth_optimism_forks::OpHardforks,
        > + StateProviderFactory,
    >,
{
    /// Calculate the next block base fee using Optimism-specific logic
    ///
    /// This function handles the Optimism-specific base fee calculation that considers:
    /// 1. Holocene hardfork activation and `extra_data` decoding
    /// 2. Optimism-specific base fee parameters
    /// 3. Fallback to standard Ethereum base fee calculation when appropriate
    fn calculate_optimism_next_block_base_fee(
        &self,
        header: &impl BlockHeader,
        timestamp: u64,
    ) -> Result<u64, <Self as reth_rpc_eth_api::EthApiTypes>::Error> {
        // Use Optimism-specific next_block_base_fee calculation
        // This handles Holocene hardfork logic and extra_data decoding
        match optimism_next_block_base_fee(
            self.inner.eth_api.provider().chain_spec(),
            header,
            timestamp,
        ) {
            Ok(base_fee) => Ok(base_fee),
            Err(e) => {
                tracing::warn!("Failed to calculate Optimism next block base fee: {:?}", e);
                // Fallback to standard calculation if Optimism-specific calculation fails
                Ok(header
                    .next_block_base_fee(
                        self.inner
                            .eth_api
                            .provider()
                            .chain_spec()
                            .base_fee_params_at_timestamp(timestamp),
                    )
                    .unwrap_or_default())
            }
        }
    }
    /// Optimism-specific gas price suggestion algorithm
    ///
    /// This implements the same algorithm as op-geth's `SuggestOptimismPriorityFee`:
    /// 1. Start with minimum suggested priority fee from config (default 0.0001 gwei)
    /// 2. Check if the last block is at capacity
    /// 3. If at capacity, return median + 10% of previous block's effective tips
    /// 4. Otherwise, return the minimum suggestion
    async fn suggest_optimism_priority_fee(
        &self,
    ) -> Result<U256, <Self as reth_rpc_eth_api::EthApiTypes>::Error> {
        let min_suggestion = self
            .inner
            .eth_api
            .gas_oracle()
            .config()
            .min_suggested_priority_fee
            .unwrap_or(DEFAULT_MIN_SUGGESTED_PRIORITY_FEE);

        // Fetch the latest block header
        let header = self
            .inner
            .eth_api
            .provider()
            .latest_header()
            .map_err(<Self as reth_rpc_eth_api::EthApiTypes>::Error::from_eth_err)?
            .ok_or(EthApiError::HeaderNotFound(BlockNumberOrTag::Latest.into()))?;

        // Check if block is at capacity
        let receipts = self
            .inner
            .eth_api
            .provider()
            .receipts_by_block(alloy_eips::HashOrNumber::Hash(header.hash()))
            .map_err(<Self as reth_rpc_eth_api::EthApiTypes>::Error::from_eth_err)?;

        // Calculate suggestion based on receipts, min suggestion if None
        let suggestion = receipts
            .and_then(|receipts| {
                // Calculate max gas usage per transaction
                let max_tx_gas = receipts
                    .windows(2)
                    .map(|w| w[1].cumulative_gas_used() - w[0].cumulative_gas_used())
                    .chain(receipts.first().map(|r| r.cumulative_gas_used()).into_iter())
                    .max()
                    .unwrap_or(0);

                // Sanity check the max gas used value
                if max_tx_gas > header.gas_limit() {
                    tracing::error!(
                        "found tx consuming more gas than the block limit: {}",
                        max_tx_gas
                    );
                    return None;
                }

                // Check if block is at capacity, if not, return None
                if header.gas_used() + max_tx_gas <= header.gas_limit() {
                    return None;
                }

                tracing::debug!("Block is at capacity, calculating median + 10%");

                // Get block for tip calculation
                let block = match self
                    .inner
                    .eth_api
                    .provider()
                    .block_by_hash(header.hash())
                    .map_err(<Self as reth_rpc_eth_api::EthApiTypes>::Error::from_eth_err)
                {
                    Ok(Some(block)) => block,
                    Ok(None) | Err(_) => return None,
                };

                let base_fee = block.header().base_fee_per_gas().unwrap_or_default();
                let mut tips: Vec<U256> = block
                    .body()
                    .transactions_iter()
                    .filter_map(|tx| tx.effective_tip_per_gas(base_fee).map(U256::from))
                    .collect();

                if tips.is_empty() {
                    tracing::error!("block was at capacity but doesn't have transactions");
                    None
                } else {
                    tips.sort_unstable();
                    let median = tips[tips.len() / 2];
                    let new_suggestion = median + median / U256::from(10);
                    Some(new_suggestion.max(min_suggestion))
                }
            })
            .unwrap_or(min_suggestion);

        // The suggestion should be capped by oracle.maxPrice
        let final_suggestion = self
            .inner
            .eth_api
            .gas_oracle()
            .config()
            .max_price
            .map(|max_price| suggestion.min(max_price))
            .unwrap_or(suggestion);

        Ok(final_suggestion)
    }
}

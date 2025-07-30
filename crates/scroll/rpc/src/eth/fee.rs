use crate::{eth::ScrollNodeCore, ScrollEthApi};
use alloy_consensus::BlockHeader;
use alloy_eips::eip7840::BlobParams;
use alloy_primitives::{Sealable, U256};
use alloy_rpc_types_eth::{BlockNumberOrTag, FeeHistory};
use futures::Future;
use reth_chainspec::EthChainSpec;
use reth_primitives_traits::BlockBody;
use reth_provider::{
    BaseFeeProvider, BlockIdReader, ChainSpecProvider, HeaderProvider, ProviderHeader,
    StateProviderFactory,
};
use reth_rpc_eth_api::{
    helpers::{EthFees, LoadFee},
    FromEthApiError, RpcNodeCore, RpcNodeCoreExt,
};
use reth_rpc_eth_types::{fee_history::calculate_reward_percentiles_for_block, EthApiError};
use reth_scroll_chainspec::{ChainConfig, ScrollChainConfig};
use reth_scroll_evm::ScrollBaseFeeProvider;
use scroll_alloy_hardforks::ScrollHardforks;
use tracing::debug;

impl<N, NetworkT> EthFees for ScrollEthApi<N, NetworkT>
where
    Self: LoadFee<
        Provider: StateProviderFactory
                      + ChainSpecProvider<
            ChainSpec: EthChainSpec<Header = ProviderHeader<Self::Provider>>
                           + ScrollHardforks
                           + ChainConfig<Config = ScrollChainConfig>,
        >,
    >,
    N: ScrollNodeCore,
{
    #[allow(clippy::manual_async_fn)]
    fn fee_history(
        &self,
        mut block_count: u64,
        mut newest_block: BlockNumberOrTag,
        reward_percentiles: Option<Vec<f64>>,
    ) -> impl Future<Output = Result<FeeHistory, Self::Error>> + Send {
        async move {
            if block_count == 0 {
                return Ok(FeeHistory::default())
            }

            // ensure the given reward percentiles aren't excessive
            if reward_percentiles.as_ref().map(|perc| perc.len() as u64) >
                Some(self.gas_oracle().config().max_reward_percentile_count)
            {
                return Err(EthApiError::InvalidRewardPercentiles.into())
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
            }

            let end_block = self
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
                    return Err(EthApiError::InvalidRewardPercentiles.into())
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

            let chain_spec = self.provider().chain_spec();
            let base_fee_provider = ScrollBaseFeeProvider::new(chain_spec.clone());

            // Check if the requested range is within the cache bounds
            let fee_entries = self.fee_history_cache().get_history(start_block, end_block).await;

            if let Some(fee_entries) = fee_entries {
                if fee_entries.len() != block_count as usize {
                    return Err(EthApiError::InvalidBlockRange.into())
                }

                for entry in &fee_entries {
                    base_fee_per_gas
                        .push(entry.header.base_fee_per_gas().unwrap_or_default() as u128);
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
                let mut provider = self
                    .provider()
                    .state_by_block_id(last_entry.header.hash_slow().into())
                    .map_err(Into::<EthApiError>::into)?;
                base_fee_per_gas.push(
                    base_fee_provider
                        .next_block_base_fee(
                            &mut provider,
                            &last_entry.header,
                            last_entry.header.timestamp(),
                        )
                        .map_err(Into::<EthApiError>::into)? as u128,
                );

                base_fee_per_blob_gas.push(last_entry.next_block_blob_fee().unwrap_or_default());
            } else {
                // read the requested header range
                let headers = self.provider()
                    .sealed_headers_range(start_block..=end_block)
                    .map_err(Self::Error::from_eth_err)?;
                if headers.len() != block_count as usize {
                    return Err(EthApiError::InvalidBlockRange.into())
                }

                for header in &headers {
                    base_fee_per_gas.push(header.base_fee_per_gas().unwrap_or_default() as u128);
                    gas_used_ratio.push(header.gas_used() as f64 / header.gas_limit() as f64);

                    let blob_params = chain_spec
                        .blob_params_at_timestamp(header.timestamp())
                        .unwrap_or_else(BlobParams::cancun);

                    base_fee_per_blob_gas.push(header.blob_fee(blob_params).unwrap_or_default());
                    blob_gas_used_ratio.push(
                        header.blob_gas_used().unwrap_or_default() as f64
                            / blob_params.max_blob_gas_per_block() as f64,
                    );

                    // Percentiles were specified, so we need to collect reward percentile info
                    if let Some(percentiles) = &reward_percentiles {
                        let (block, receipts) = self.cache()
                            .get_block_and_receipts(header.hash())
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
                let mut provider = self
                    .provider()
                    .state_by_block_id(last_header.hash().into())
                    .map_err(Into::<EthApiError>::into)?;
                base_fee_per_gas.push(
                    base_fee_provider
                        .next_block_base_fee(
                            &mut provider,
                            &last_header.header(),
                            last_header.timestamp(),
                        )
                        .map_err(Into::<EthApiError>::into)? as u128,
                );
                // Same goes for the `base_fee_per_blob_gas`:
                // > "[..] includes the next block after the newest of the returned range, because this value can be derived from the newest block.
                base_fee_per_blob_gas.push(
                    last_header
                    .maybe_next_block_blob_fee(
                        chain_spec.blob_params_at_timestamp(last_header.timestamp())
                    ).unwrap_or_default()
                );
            };

            // Scroll-specific logic: update rewards if the newest_block is not at capacity and tip
            // calculation succeeds
            let (suggest_tip_cap_result, is_at_capacity) = self
                .gas_oracle()
                .calculate_suggest_tip_cap(
                    newest_block,
                    U256::from(self.inner.min_suggested_priority_fee),
                    self.inner.payload_size_limit,
                )
                .await;

            let reward = match (is_at_capacity, suggest_tip_cap_result) {
                (false, Ok(suggest_tip_cap_value)) => {
                    let suggest_tip_cap = suggest_tip_cap_value.saturating_to::<u128>();
                    reward_percentiles.map(|percentiles| {
                        vec![vec![suggest_tip_cap; percentiles.len()]; block_count as usize]
                    })
                }
                _ => reward_percentiles.map(|_| rewards),
            };

            Ok(FeeHistory {
                base_fee_per_gas,
                gas_used_ratio,
                base_fee_per_blob_gas,
                blob_gas_used_ratio,
                oldest_block: start_block,
                reward,
            })
        }
    }
}

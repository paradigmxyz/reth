//! Loads fee history from database. Helper trait for `eth_` fee and transaction RPC methods.

use alloy_primitives::U256;
use alloy_rpc_types::{BlockNumberOrTag, FeeHistory};
use futures::Future;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_provider::{BlockIdReader, BlockReaderIdExt, ChainSpecProvider, HeaderProvider};
use reth_rpc_eth_types::{
    fee_history::calculate_reward_percentiles_for_block, EthApiError, EthStateCache,
    FeeHistoryCache, FeeHistoryEntry, GasPriceOracle, RpcInvalidTransactionError,
};
use tracing::debug;

use crate::FromEthApiError;

use super::LoadBlock;

/// Fee related functions for the [`EthApiServer`](crate::EthApiServer) trait in the
/// `eth_` namespace.
pub trait EthFees: LoadFee {
    /// Returns a suggestion for a gas price for legacy transactions.
    ///
    /// See also: <https://github.com/ethereum/pm/issues/328#issuecomment-853234014>
    fn gas_price(&self) -> impl Future<Output = Result<U256, Self::Error>> + Send
    where
        Self: LoadBlock,
    {
        LoadFee::gas_price(self)
    }

    /// Returns a suggestion for a base fee for blob transactions.
    fn blob_base_fee(&self) -> impl Future<Output = Result<U256, Self::Error>> + Send
    where
        Self: LoadBlock,
    {
        LoadFee::blob_base_fee(self)
    }

    /// Returns a suggestion for the priority fee (the tip)
    fn suggested_priority_fee(&self) -> impl Future<Output = Result<U256, Self::Error>> + Send
    where
        Self: 'static,
    {
        LoadFee::suggested_priority_fee(self)
    }

    /// Reports the fee history, for the given amount of blocks, up until the given newest block.
    ///
    /// If `reward_percentiles` are provided the [`FeeHistory`] will include the _approximated_
    /// rewards for the requested range.
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

            let end_block = LoadFee::provider(self)
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
            // otherwise `newest_block - 2
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
                    return Err(EthApiError::InvalidBlockRange.into())
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
                base_fee_per_gas
                    .push(last_entry.next_block_base_fee(LoadFee::provider(self).chain_spec())
                        as u128);

                base_fee_per_blob_gas.push(last_entry.next_block_blob_fee().unwrap_or_default());
            } else {
                // read the requested header range
                let headers = LoadFee::provider(self)
                    .sealed_headers_range(start_block..=end_block)
                    .map_err(Self::Error::from_eth_err)?;
                if headers.len() != block_count as usize {
                    return Err(EthApiError::InvalidBlockRange.into())
                }


                for header in &headers {
                    base_fee_per_gas.push(header.base_fee_per_gas.unwrap_or_default() as u128);
                    gas_used_ratio.push(header.gas_used as f64 / header.gas_limit as f64);
                    base_fee_per_blob_gas.push(header.blob_fee().unwrap_or_default());
                    blob_gas_used_ratio.push(
                        header.blob_gas_used.unwrap_or_default() as f64
                            / reth_primitives::constants::eip4844::MAX_DATA_GAS_PER_BLOCK as f64,
                    );

                    // Percentiles were specified, so we need to collect reward percentile ino
                    if let Some(percentiles) = &reward_percentiles {
                        let (transactions, receipts) = LoadFee::cache(self)
                            .get_transactions_and_receipts(header.hash())
                            .await
                            .map_err(Self::Error::from_eth_err)?
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
                let last_header = headers.last().expect("is present");
                base_fee_per_gas.push(
                    LoadFee::provider(self)
                        .chain_spec()
                        .base_fee_params_at_timestamp(last_header.timestamp)
                        .next_block_base_fee(
                            last_header.gas_used ,
                            last_header.gas_limit,
                            last_header.base_fee_per_gas.unwrap_or_default() ,
                        ) as u128,
                );

                // Same goes for the `base_fee_per_blob_gas`:
                // > "[..] includes the next block after the newest of the returned range, because this value can be derived from the newest block.
                base_fee_per_blob_gas.push(last_header.next_block_blob_fee().unwrap_or_default());
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

    /// Approximates reward at a given percentile for a specific block
    /// Based on the configured resolution
    fn approximate_percentile(&self, entry: &FeeHistoryEntry, requested_percentile: f64) -> u128 {
        let resolution = self.fee_history_cache().resolution();
        let rounded_percentile =
            (requested_percentile * resolution as f64).round() / resolution as f64;
        let clamped_percentile = rounded_percentile.clamp(0.0, 100.0);

        // Calculate the index in the precomputed rewards array
        let index = (clamped_percentile / (1.0 / resolution as f64)).round() as usize;
        // Fetch the reward from the FeeHistoryEntry
        entry.rewards.get(index).copied().unwrap_or_default()
    }
}

/// Loads fee from database.
///
/// Behaviour shared by several `eth_` RPC methods, not exclusive to `eth_` fees RPC methods.
pub trait LoadFee: LoadBlock {
    // Returns a handle for reading data from disk.
    ///
    /// Data access in default (L1) trait method implementations.
    fn provider(
        &self,
    ) -> impl BlockIdReader + HeaderProvider + ChainSpecProvider<ChainSpec: EthereumHardforks>;

    /// Returns a handle for reading data from memory.
    ///
    /// Data access in default (L1) trait method implementations.
    fn cache(&self) -> &EthStateCache;

    /// Returns a handle for reading gas price.
    ///
    /// Data access in default (L1) trait method implementations.
    fn gas_oracle(&self) -> &GasPriceOracle<impl BlockReaderIdExt>;

    /// Returns a handle for reading fee history data from memory.
    ///
    /// Data access in default (L1) trait method implementations.
    fn fee_history_cache(&self) -> &FeeHistoryCache;

    /// Returns the gas price if it is set, otherwise fetches a suggested gas price for legacy
    /// transactions.
    fn legacy_gas_price(
        &self,
        gas_price: Option<U256>,
    ) -> impl Future<Output = Result<U256, Self::Error>> + Send {
        async move {
            match gas_price {
                Some(gas_price) => Ok(gas_price),
                None => {
                    // fetch a suggested gas price
                    self.gas_price().await
                }
            }
        }
    }

    /// Returns the EIP-1559 fees if they are set, otherwise fetches a suggested gas price for
    /// EIP-1559 transactions.
    ///
    /// Returns (`max_fee`, `priority_fee`)
    fn eip1559_fees(
        &self,
        max_fee_per_gas: Option<U256>,
        max_priority_fee_per_gas: Option<U256>,
    ) -> impl Future<Output = Result<(U256, U256), Self::Error>> + Send {
        async move {
            let max_fee_per_gas = match max_fee_per_gas {
                Some(max_fee_per_gas) => max_fee_per_gas,
                None => {
                    // fetch pending base fee
                    let base_fee = self
                        .block(BlockNumberOrTag::Pending.into())
                        .await?
                        .ok_or(EthApiError::HeaderNotFound(BlockNumberOrTag::Pending.into()))?
                        .base_fee_per_gas
                        .ok_or(EthApiError::InvalidTransaction(
                            RpcInvalidTransactionError::TxTypeNotSupported,
                        ))?;
                    U256::from(base_fee)
                }
            };

            let max_priority_fee_per_gas = match max_priority_fee_per_gas {
                Some(max_priority_fee_per_gas) => max_priority_fee_per_gas,
                None => self.suggested_priority_fee().await?,
            };
            Ok((max_fee_per_gas, max_priority_fee_per_gas))
        }
    }

    /// Returns the EIP-4844 blob fee if it is set, otherwise fetches a blob fee.
    fn eip4844_blob_fee(
        &self,
        blob_fee: Option<U256>,
    ) -> impl Future<Output = Result<U256, Self::Error>> + Send {
        async move {
            match blob_fee {
                Some(blob_fee) => Ok(blob_fee),
                None => self.blob_base_fee().await,
            }
        }
    }

    /// Returns a suggestion for a gas price for legacy transactions.
    ///
    /// See also: <https://github.com/ethereum/pm/issues/328#issuecomment-853234014>
    fn gas_price(&self) -> impl Future<Output = Result<U256, Self::Error>> + Send {
        let header = self.block(BlockNumberOrTag::Latest.into());
        let suggested_tip = self.suggested_priority_fee();
        async move {
            let (header, suggested_tip) = futures::try_join!(header, suggested_tip)?;
            let base_fee = header.and_then(|h| h.base_fee_per_gas).unwrap_or_default();
            Ok(suggested_tip + U256::from(base_fee))
        }
    }

    /// Returns a suggestion for a base fee for blob transactions.
    fn blob_base_fee(&self) -> impl Future<Output = Result<U256, Self::Error>> + Send {
        async move {
            self.block(BlockNumberOrTag::Latest.into())
                .await?
                .and_then(|h: reth_primitives::SealedBlock| h.next_block_blob_fee())
                .ok_or(EthApiError::ExcessBlobGasNotSet.into())
                .map(U256::from)
        }
    }

    /// Returns a suggestion for the priority fee (the tip)
    fn suggested_priority_fee(&self) -> impl Future<Output = Result<U256, Self::Error>> + Send
    where
        Self: 'static,
    {
        async move { self.gas_oracle().suggest_tip_cap().await.map_err(Self::Error::from_eth_err) }
    }
}

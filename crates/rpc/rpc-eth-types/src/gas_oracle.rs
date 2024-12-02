//! An implementation of the eth gas price oracle, used for providing gas price estimates based on
//! previous blocks.

use alloy_consensus::{constants::GWEI_TO_WEI, BlockHeader};
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::{B256, U256};
use alloy_rpc_types_eth::BlockId;
use derive_more::{Deref, DerefMut, From, Into};
use itertools::Itertools;
use reth_primitives_traits::SignedTransaction;
use reth_rpc_server_types::{
    constants,
    constants::gas_oracle::{
        DEFAULT_GAS_PRICE_BLOCKS, DEFAULT_GAS_PRICE_PERCENTILE, DEFAULT_IGNORE_GAS_PRICE,
        DEFAULT_MAX_GAS_PRICE, MAX_HEADER_HISTORY, SAMPLE_NUMBER,
    },
};
use reth_storage_api::BlockReaderIdExt;
use schnellru::{ByLength, LruMap};
use serde::{Deserialize, Serialize};
use std::fmt::{self, Debug, Formatter};
use tokio::sync::Mutex;
use tracing::warn;

use super::{EthApiError, EthResult, EthStateCache, RpcInvalidTransactionError};

/// The default gas limit for `eth_call` and adjacent calls. See
/// [`RPC_DEFAULT_GAS_CAP`](constants::gas_oracle::RPC_DEFAULT_GAS_CAP).
pub const RPC_DEFAULT_GAS_CAP: GasCap = GasCap(constants::gas_oracle::RPC_DEFAULT_GAS_CAP);

/// Settings for the [`GasPriceOracle`]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GasPriceOracleConfig {
    /// The number of populated blocks to produce the gas price estimate
    pub blocks: u32,

    /// The percentile of gas prices to use for the estimate
    pub percentile: u32,

    /// The maximum number of headers to keep in the cache
    pub max_header_history: u64,

    /// The maximum number of blocks for estimating gas price
    pub max_block_history: u64,

    /// The default gas price to use if there are no blocks to use
    pub default: Option<U256>,

    /// The maximum gas price to use for the estimate
    pub max_price: Option<U256>,

    /// The minimum gas price, under which the sample will be ignored
    pub ignore_price: Option<U256>,
}

impl Default for GasPriceOracleConfig {
    fn default() -> Self {
        Self {
            blocks: DEFAULT_GAS_PRICE_BLOCKS,
            percentile: DEFAULT_GAS_PRICE_PERCENTILE,
            max_header_history: MAX_HEADER_HISTORY,
            max_block_history: MAX_HEADER_HISTORY,
            default: None,
            max_price: Some(DEFAULT_MAX_GAS_PRICE),
            ignore_price: Some(DEFAULT_IGNORE_GAS_PRICE),
        }
    }
}

/// Calculates a gas price depending on recent blocks.
#[derive(Debug)]
pub struct GasPriceOracle<Provider> {
    /// The type used to subscribe to block events and get block info
    provider: Provider,
    /// The cache for blocks
    cache: EthStateCache,
    /// The config for the oracle
    oracle_config: GasPriceOracleConfig,
    /// The price under which the sample will be ignored.
    ignore_price: Option<u128>,
    /// Stores the latest calculated price and its block hash and Cache stores the lowest effective
    /// tip values of recent blocks
    inner: Mutex<GasPriceOracleInner>,
}

impl<Provider> GasPriceOracle<Provider>
where
    Provider: BlockReaderIdExt,
{
    /// Creates and returns the [`GasPriceOracle`].
    pub fn new(
        provider: Provider,
        mut oracle_config: GasPriceOracleConfig,
        cache: EthStateCache,
    ) -> Self {
        // sanitize the percentile to be less than 100
        if oracle_config.percentile > 100 {
            warn!(prev_percentile = ?oracle_config.percentile, "Invalid configured gas price percentile, assuming 100.");
            oracle_config.percentile = 100;
        }
        let ignore_price = oracle_config.ignore_price.map(|price| price.saturating_to());

        // this is the number of blocks that we will cache the values for
        let cached_values = (oracle_config.blocks * 5).max(oracle_config.max_block_history as u32);
        let inner = Mutex::new(GasPriceOracleInner {
            last_price: Default::default(),
            lowest_effective_tip_cache: EffectiveTipLruCache(LruMap::new(ByLength::new(
                cached_values,
            ))),
        });

        Self { provider, oracle_config, cache, ignore_price, inner }
    }

    /// Returns the configuration of the gas price oracle.
    pub const fn config(&self) -> &GasPriceOracleConfig {
        &self.oracle_config
    }

    /// Suggests a gas price estimate based on recent blocks, using the configured percentile.
    pub async fn suggest_tip_cap(&self) -> EthResult<U256> {
        let header = self
            .provider
            .sealed_header_by_number_or_tag(BlockNumberOrTag::Latest)?
            .ok_or(EthApiError::HeaderNotFound(BlockId::latest()))?;

        let mut inner = self.inner.lock().await;

        // if we have stored a last price, then we check whether or not it was for the same head
        if inner.last_price.block_hash == header.hash() {
            return Ok(inner.last_price.price)
        }

        // if all responses are empty, then we can return a maximum of 2*check_block blocks' worth
        // of prices
        //
        // we only return more than check_block blocks' worth of prices if one or more return empty
        // transactions
        let mut current_hash = header.hash();
        let mut results = Vec::new();
        let mut populated_blocks = 0;

        // we only check a maximum of 2 * max_block_history, or the number of blocks in the chain
        let max_blocks = if self.oracle_config.max_block_history * 2 > header.number() {
            header.number()
        } else {
            self.oracle_config.max_block_history * 2
        };

        for _ in 0..max_blocks {
            // Check if current hash is in cache
            let (parent_hash, block_values) =
                if let Some(vals) = inner.lowest_effective_tip_cache.get(&current_hash) {
                    vals.to_owned()
                } else {
                    // Otherwise we fetch it using get_block_values
                    let (parent_hash, block_values) = self
                        .get_block_values(current_hash, SAMPLE_NUMBER)
                        .await?
                        .ok_or(EthApiError::HeaderNotFound(current_hash.into()))?;
                    inner
                        .lowest_effective_tip_cache
                        .insert(current_hash, (parent_hash, block_values.clone()));
                    (parent_hash, block_values)
                };

            if block_values.is_empty() {
                results.push(U256::from(inner.last_price.price));
            } else {
                results.extend(block_values);
                populated_blocks += 1;
            }

            // break when we have enough populated blocks
            if populated_blocks >= self.oracle_config.blocks {
                break
            }

            current_hash = parent_hash;
        }

        // sort results then take the configured percentile result
        let mut price = if results.is_empty() {
            inner.last_price.price
        } else {
            results.sort_unstable();
            *results.get((results.len() - 1) * self.oracle_config.percentile as usize / 100).expect(
                "gas price index is a percent of nonzero array length, so a value always exists",
            )
        };

        // constrain to the max price
        if let Some(max_price) = self.oracle_config.max_price {
            if price > max_price {
                price = max_price;
            }
        }

        inner.last_price = GasPriceOracleResult { block_hash: header.hash(), price };

        Ok(price)
    }

    /// Get the `limit` lowest effective tip values for the given block. If the oracle has a
    /// configured `ignore_price` threshold, then tip values under that threshold will be ignored
    /// before returning a result.
    ///
    /// If the block cannot be found, then this will return `None`.
    ///
    /// This method also returns the parent hash for the given block.
    async fn get_block_values(
        &self,
        block_hash: B256,
        limit: usize,
    ) -> EthResult<Option<(B256, Vec<U256>)>> {
        // check the cache (this will hit the disk if the block is not cached)
        let block = match self.cache.get_sealed_block_with_senders(block_hash).await? {
            Some(block) => block,
            None => return Ok(None),
        };

        let base_fee_per_gas = block.base_fee_per_gas;
        let parent_hash = block.parent_hash;

        // sort the functions by ascending effective tip first
        let sorted_transactions = block
            .body
            .transactions
            .iter()
            .sorted_by_cached_key(|tx| tx.effective_tip_per_gas(base_fee_per_gas));

        let mut prices = Vec::with_capacity(limit);

        for tx in sorted_transactions {
            let mut effective_gas_tip = None;
            // ignore transactions with a tip under the configured threshold
            if let Some(ignore_under) = self.ignore_price {
                let tip = tx.effective_tip_per_gas(base_fee_per_gas);
                effective_gas_tip = Some(tip);
                if tip < Some(ignore_under) {
                    continue
                }
            }

            // check if the sender was the coinbase, if so, ignore
            if let Some(sender) = tx.recover_signer() {
                if sender == block.beneficiary {
                    continue
                }
            }

            // a `None` effective_gas_tip represents a transaction where the max_fee_per_gas is
            // less than the base fee which would be invalid
            let effective_gas_tip = effective_gas_tip
                .unwrap_or_else(|| tx.effective_tip_per_gas(base_fee_per_gas))
                .ok_or(RpcInvalidTransactionError::FeeCapTooLow)?;

            prices.push(U256::from(effective_gas_tip));

            // we have enough entries
            if prices.len() >= limit {
                break
            }
        }

        Ok(Some((parent_hash, prices)))
    }
}

/// Container type for mutable inner state of the [`GasPriceOracle`]
#[derive(Debug)]
struct GasPriceOracleInner {
    last_price: GasPriceOracleResult,
    lowest_effective_tip_cache: EffectiveTipLruCache,
}

/// Wrapper struct for `LruMap`
#[derive(Deref, DerefMut)]
pub struct EffectiveTipLruCache(LruMap<B256, (B256, Vec<U256>), ByLength>);

impl Debug for EffectiveTipLruCache {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("EffectiveTipLruCache")
            .field("cache_length", &self.len())
            .field("cache_memory_usage", &self.memory_usage())
            .finish()
    }
}

/// Stores the last result that the oracle returned
#[derive(Debug, Clone)]
pub struct GasPriceOracleResult {
    /// The block hash that the oracle used to calculate the price
    pub block_hash: B256,
    /// The price that the oracle calculated
    pub price: U256,
}

impl Default for GasPriceOracleResult {
    fn default() -> Self {
        Self { block_hash: B256::ZERO, price: U256::from(GWEI_TO_WEI) }
    }
}

/// The wrapper type for gas limit
#[derive(Debug, Clone, Copy, From, Into)]
pub struct GasCap(pub u64);

impl Default for GasCap {
    fn default() -> Self {
        RPC_DEFAULT_GAS_CAP
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_price_sanity() {
        assert_eq!(DEFAULT_MAX_GAS_PRICE, U256::from(500_000_000_000u64));
        assert_eq!(DEFAULT_MAX_GAS_PRICE, U256::from(500 * GWEI_TO_WEI))
    }

    #[test]
    fn ignore_price_sanity() {
        assert_eq!(DEFAULT_IGNORE_GAS_PRICE, U256::from(2u64));
    }
}

//! An implementation of the eth gas price oracle, used for providing gas price estimates based on
//! previous blocks.
use super::{cache::EthStateCache, error::EthResult};
use reth_interfaces::{Error as GeneralError, Result};
use reth_primitives::{
    constants::GWEI_TO_WEI, BlockHashOrNumber, BlockId, BlockNumberOrTag, H256, U256,
};
use reth_provider::{BlockProviderIdExt, ProviderError};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::Mutex;

/// The number of transactions sampled in a block
pub const SAMPLE_NUMBER: u32 = 3;

/// The default maximum gas price to use for the estimate
pub const DEFAULT_MAX_PRICE: U256 = U256::from_limbs([500_000_000_000u64, 0, 0, 0]);

/// The default minimum gas price, under which the sample will be ignored
pub const DEFAULT_IGNORE_PRICE: U256 = U256::from_limbs([2u64, 0, 0, 0]);

/// Settings for the [GasPriceOracle]
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GasPriceOracleConfig {
    /// The number of blocks to sample for gas price estimates
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
        GasPriceOracleConfig {
            blocks: 20,
            percentile: 60,
            max_header_history: 1024,
            max_block_history: 1024,
            default: None,
            max_price: Some(DEFAULT_MAX_PRICE),
            ignore_price: Some(DEFAULT_IGNORE_PRICE),
        }
    }
}

/// An error that can occur when calculating the gas price estimates
#[derive(Debug, Clone, Error)]
pub enum GasPriceOracleError {
    /// A transaction failed sender recovery
    #[error("transaction failed sender recovery")]
    InvalidSignature,
}

/// Calculates a gas price depending on recent blocks.
#[derive(Debug)]
pub struct GasPriceOracle<Client> {
    /// The type used to subscribe to block events and get block info
    client: Client,
    /// The cache for blocks
    cache: EthStateCache,
    /// The config for the oracle
    oracle_config: GasPriceOracleConfig,
    /// The latest calculated price and its block hash
    last_price: Mutex<GasPriceOracleResult>,
}

impl<Client> GasPriceOracle<Client>
where
    Client: BlockProviderIdExt + 'static,
{
    /// Creates and returns the [GasPriceOracle].
    pub fn new(client: Client, oracle_config: GasPriceOracleConfig, cache: EthStateCache) -> Self {
        Self { client, oracle_config, last_price: Default::default(), cache }
    }

    /// Suggests a gas price estimate based on recent blocks, using the configured percentile.
    pub async fn suggest_tip_cap(&self) -> EthResult<U256> {
        // TODO: we really shouldn't have this be None, but let's get rid of these expects?
        let header_hash = self
            .client
            .block_hash_for_id(BlockId::Number(BlockNumberOrTag::Latest))?
            .expect("a latest header always exists");
        let header = self
            .client
            .header_by_hash_or_number(BlockHashOrNumber::Hash(header_hash))?
            .ok_or(GeneralError::Provider(ProviderError::BlockHash { block_hash: header_hash }))?;

        let mut last_price = self.last_price.lock().await;

        // if we have stored a last price, then we check whether or not it was for the same head
        if last_price.block_hash == header_hash {
            return Ok(last_price.price)
        }

        // if all responses are empty, then we can return a maximum of 2*check_block blocks' worth
        // of prices
        //
        // we only return more than check_block blocks' worth of prices if one or more return empty
        // transactions
        let mut current_block = header.number;
        let mut results = Vec::new();
        let mut populated_blocks = 0;

        // we only check a maximum of 2 * max_block_history
        for _ in 0..self.oracle_config.max_block_history * 2 {
            // TODO - error handling
            let block_values = self
                .get_block_values(current_block, SAMPLE_NUMBER as usize)
                .await?
                .expect("this means we couldn't find the block");
            if block_values.is_empty() {
                results.push(Some(U256::from(last_price.price)));
            } else {
                results.extend(block_values);
                populated_blocks += 1;
            }

            if populated_blocks >= self.oracle_config.max_block_history {
                break
            }

            current_block -= 1;
        }

        // sort results then take the configured percentile result
        let mut price = last_price.price;
        if !results.is_empty() {
            results.sort_unstable();
            // TODO - error handling
            price = results
                .get((results.len() - 1) * self.oracle_config.percentile as usize / 100)
                .expect("this should exist")
                .expect("this shouldn't be None");
        }

        // constrain to the max price
        if let Some(max_price) = self.oracle_config.max_price {
            price = max_price;
        }

        *last_price = GasPriceOracleResult { block_hash: header_hash, price };

        Ok(price)
    }

    async fn get_block_values(
        &self,
        block_num: u64,
        limit: usize,
    ) -> Result<Option<Vec<Option<U256>>>> {
        // TODO: we could cache num -> hash as well as long as we invalidate the cache between
        // forkchoice updates
        let block_hash = match self.client.block_hash(block_num)? {
            Some(num) => num,
            None => return Ok(None),
        };

        // check the cache
        let block = match self.cache.get_block(block_hash).await? {
            Some(block) => block,
            None => match self.client.block_by_number(block_num)? {
                Some(block) => block,
                None => return Ok(None),
            },
        };

        // sort the transactions by effective tip
        // but first filter those that should be ignored
        let txs = block.body.iter();
        let mut txs = txs
            .filter(|tx| {
                if let Some(ignore_under) = self.oracle_config.ignore_price {
                    if tx.effective_gas_tip(block.base_fee_per_gas).map(U256::from) <
                        Some(ignore_under)
                    {
                        return false
                    }
                }

                // recover sender, check if coinbase
                let sender = tx.recover_signer();
                match sender {
                    Some(addr) => addr == block.beneficiary,
                    // TODO: figure out an error for this case or ignore
                    None => false,
                }
            })
            .collect::<Vec<_>>();

        // now do the sort
        txs.sort_unstable_by(|first, second| {
            first
                .effective_gas_tip(block.base_fee_per_gas)
                .cmp(&second.effective_gas_tip(block.base_fee_per_gas))
        });

        Ok(Some(
            txs.iter_mut()
                .take(limit)
                .map(|tx| tx.effective_gas_tip(block.base_fee_per_gas).map(U256::from))
                .collect(),
        ))
    }
}

/// Stores the last result that the oracle returned
#[derive(Debug, Clone)]
pub struct GasPriceOracleResult {
    /// The block hash that the oracle used to calculate the price
    pub block_hash: H256,
    /// The price that the oracle calculated
    pub price: U256,
}

impl Default for GasPriceOracleResult {
    fn default() -> Self {
        Self { block_hash: H256::zero(), price: U256::from(GWEI_TO_WEI) }
    }
}

#[cfg(test)]
mod tests {
    use reth_primitives::constants::GWEI_TO_WEI;

    use super::*;

    #[test]
    fn max_price_sanity() {
        assert_eq!(DEFAULT_MAX_PRICE, U256::from(500_000_000_000u64));
        assert_eq!(DEFAULT_MAX_PRICE, U256::from(500 * GWEI_TO_WEI))
    }

    #[test]
    fn ignore_price_sanity() {
        assert_eq!(DEFAULT_IGNORE_PRICE, U256::from(2u64));
    }
}

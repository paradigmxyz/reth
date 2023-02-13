use lru::LruCache;
use reth_primitives::{BlockNumber, U256};
use serde::{Deserialize, Serialize};

/// Response type for `eth_feeHistory`
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeeHistory {
    /// An array of block base fees per gas.
    /// This includes the next block after the newest of the returned range,
    /// because this value can be derived from the newest block. Zeroes are
    /// returned for pre-EIP-1559 blocks.
    pub base_fee_per_gas: Vec<U256>,
    /// An array of block gas used ratios. These are calculated as the ratio
    /// of `gasUsed` and `gasLimit`.
    pub gas_used_ratio: Vec<f64>,
    /// Lowest number block of the returned range.
    pub oldest_block: U256,
    /// An (optional) array of effective priority fee per gas data points from a single
    /// block. All zeroes are returned if the block is empty.
    #[serde(default)]
    pub reward: Option<Vec<Vec<U256>>>,
}

/// LRU cache for `eth_feeHistory` RPC method. Block Number => Fee History.
pub type FeeHistoryCache = LruCache<BlockNumber, FeeHistoryCacheItem>;

/// [FeeHistoryCache] item.
#[derive(Clone, Debug)]
pub struct FeeHistoryCacheItem {
    /// Block base fee per gas. Zero for pre-EIP-1559 blocks.
    pub base_fee_per_gas: U256,
    /// Block gas used ratio. Calculated as the ratio of `gasUsed` and `gasLimit`.
    pub gas_used_ratio: f64,
    /// An (optional) array of effective priority fee per gas data points for a
    /// block. All zeroes are returned if the block is empty.
    pub reward: Option<Vec<U256>>,
}

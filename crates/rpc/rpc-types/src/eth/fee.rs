use lru::LruCache;
use reth_primitives::{BlockNumber, H256, U256};
use serde::{Deserialize, Serialize};
use std::{num::NonZeroUsize, sync::Arc};
use tokio::sync::Mutex;

/// Internal struct to calculate reward percentiles
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxGasAndReward {
    /// gas used by a block
    pub gas_used: u128,
    /// minimum between max_priority_fee_per_gas or max_fee_per_gas - base_fee_for_block
    pub reward: u128,
}

impl PartialOrd for TxGasAndReward {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        // compare only the reward
        // see:
        // <https://github.com/ethereum/go-ethereum/blob/ee8e83fa5f6cb261dad2ed0a7bbcde4930c41e6c/eth/gasprice/feehistory.go#L85>
        self.reward.partial_cmp(&other.reward)
    }
}

impl Ord for TxGasAndReward {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // compare only the reward
        // see:
        // <https://github.com/ethereum/go-ethereum/blob/ee8e83fa5f6cb261dad2ed0a7bbcde4930c41e6c/eth/gasprice/feehistory.go#L85>
        self.reward.cmp(&other.reward)
    }
}

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
#[derive(Clone, Debug)]
pub struct FeeHistoryCache(pub Arc<Mutex<LruCache<BlockNumber, FeeHistoryCacheItem>>>);

impl FeeHistoryCache {
    /// Creates a new LRU Cache that holds at most cap items.
    pub fn new(cap: NonZeroUsize) -> Self {
        Self(Arc::new(Mutex::new(LruCache::new(cap))))
    }
}

/// [FeeHistoryCache] item.
#[derive(Clone, Debug)]
pub struct FeeHistoryCacheItem {
    /// Block hash (`None` if it wasn't the oldest block in `eth_feeHistory` response where
    /// cache is populated)
    pub hash: Option<H256>,
    /// Block base fee per gas. Zero for pre-EIP-1559 blocks.
    pub base_fee_per_gas: U256,
    /// Block gas used ratio. Calculated as the ratio of `gasUsed` and `gasLimit`.
    pub gas_used_ratio: f64,
    /// An (optional) array of effective priority fee per gas data points for a
    /// block. All zeroes are returned if the block is empty.
    pub reward: Option<Vec<U256>>,
}

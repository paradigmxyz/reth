use reth_primitives::U256;
use serde::{Deserialize, Serialize};

/// Response type for `eth_feeHistory`
#[derive(Debug, Clone, Deserialize, Serialize)]
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

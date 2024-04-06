use alloy_primitives::U256;
use serde::{Deserialize, Deserializer, Serialize};

/// Additional `evm_mine` options
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MineOptions {
    Options {
        #[serde(deserialize_with = "deserialize_stringified_u64_opt")]
        timestamp: Option<u64>,
        // If `blocks` is given, it will mine exactly blocks number of blocks, regardless of any
        // other blocks mined or reverted during it's operation
        blocks: Option<u64>,
    },
    /// The timestamp the block should be mined with
    #[serde(deserialize_with = "deserialize_stringified_u64_opt")]
    Timestamp(Option<u64>),
}

impl Default for MineOptions {
    fn default() -> Self {
        MineOptions::Options { timestamp: None, blocks: None }
    }
}

/// Supports parsing u64
///
/// See <https://github.com/gakonst/ethers-rs/issues/1507>
fn deserialize_stringified_u64_opt<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
    where
        D: Deserializer<'de>,
{
    if let Some(num) = Option::<U256>::deserialize(deserializer)? {
        num.try_into().map(Some).map_err(serde::de::Error::custom)
    } else {
        Ok(None)
    }
}

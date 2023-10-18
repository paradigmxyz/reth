use alloy_primitives::{B256, U256, U64};
use serde::{Deserialize, Serialize};

/// This structure contains configurable settings of the transition process.
#[derive(Default, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitionConfiguration {
    /// Maps on the TERMINAL_TOTAL_DIFFICULTY parameter of EIP-3675
    pub terminal_total_difficulty: U256,
    /// Maps on TERMINAL_BLOCK_HASH parameter of EIP-3675
    pub terminal_block_hash: B256,
    /// Maps on TERMINAL_BLOCK_NUMBER parameter of EIP-3675
    pub terminal_block_number: U64,
}

use crate::engine::PayloadAttributes;
use alloy_primitives::Bytes;
use serde::{Deserialize, Serialize};

/// Optimism Payload Attributes
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptimismPayloadAttributes {
    /// The payload attributes
    #[serde(flatten)]
    pub payload_attributes: PayloadAttributes,
    /// Transactions is a field for rollups: the transactions list is forced into the block
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transactions: Option<Vec<Bytes>>,
    /// If true, the no transactions are taken out of the tx-pool, only transactions from the above
    /// Transactions list will be included.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_tx_pool: Option<bool>,
    /// If set, this sets the exact gas limit the block produced with.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::serde_helpers::u64_hex_opt::deserialize"
    )]
    pub gas_limit: Option<u64>,
}

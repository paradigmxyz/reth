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

#[cfg(test)]
mod tests {
    use crate::engine::{ExecutionPayloadInputV2, OptimismPayloadAttributes};

    // <https://github.com/paradigmxyz/reth/issues/6036>
    #[test]
    fn deserialize_op_base_payload() {
        let payload = r#"{"parentHash":"0x24e8df372a61cdcdb1a163b52aaa1785e0c869d28c3b742ac09e826bbb524723","feeRecipient":"0x4200000000000000000000000000000000000011","stateRoot":"0x9a5db45897f1ff1e620a6c14b0a6f1b3bcdbed59f2adc516a34c9a9d6baafa71","receiptsRoot":"0x8af6f74835d47835deb5628ca941d00e0c9fd75585f26dabdcb280ec7122e6af","logsBloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","prevRandao":"0xf37b24eeff594848072a05f74c8600001706c83e489a9132e55bf43a236e42ec","blockNumber":"0xe3d5d8","gasLimit":"0x17d7840","gasUsed":"0xb705","timestamp":"0x65a118c0","extraData":"0x","baseFeePerGas":"0x7a0ff32","blockHash":"0xf5c147b2d60a519b72434f0a8e082e18599021294dd9085d7597b0ffa638f1c0","withdrawals":[],"transactions":["0x7ef90159a05ba0034ffdcb246703298224564720b66964a6a69d0d7e9ffd970c546f7c048094deaddeaddeaddeaddeaddeaddeaddeaddead00019442000000000000000000000000000000000000158080830f424080b90104015d8eb900000000000000000000000000000000000000000000000000000000009e1c4a0000000000000000000000000000000000000000000000000000000065a11748000000000000000000000000000000000000000000000000000000000000000a4b479e5fa8d52dd20a8a66e468b56e993bdbffcccf729223aabff06299ab36db000000000000000000000000000000000000000000000000000000000000000400000000000000000000000073b4168cc87f35cc239200a20eb841cded23493b000000000000000000000000000000000000000000000000000000000000083400000000000000000000000000000000000000000000000000000000000f4240"]}"#;
        let _payload = serde_json::from_str::<ExecutionPayloadInputV2>(payload).unwrap();
    }

    #[test]
    fn deserialize_op_payload_attributes() {
        let payload = r#"{"prevRandao":"0x24e8df372a61cdcdb1a163b52aaa1785e0c869d28c3b742ac09e826bbb524723","suggestedFeeRecipient":"0x4200000000000000000000000000000000000011","timestamp":"1","gasLimit":"0x17d7840","transactions":[],"no_tx_pool":"true","withdrawals":[]}"#;
        let _payload = serde_json::from_str::<OptimismPayloadAttributes>(payload).unwrap();
    }
}

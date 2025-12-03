//! Testing namespace for building a block in a single call.
//!
//! This follows the `testing_buildBlockV1` specification and is intended for
//! non-production/debug use only.

use alloy_primitives::{Bytes, B256};
use alloy_rpc_types_engine::{
    ExecutionPayloadEnvelopeV4, PayloadAttributes as EthPayloadAttributes,
};
use jsonrpsee::proc_macros::rpc;
use serde::{Deserialize, Serialize};

/// Capability string for `testing_buildBlockV1`.
pub const TESTING_BUILD_BLOCK_V1: &str = "testing_buildBlockV1";

/// Request payload for `testing_buildBlockV1`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestingBuildBlockRequest {
    /// Parent block hash of the block to build.
    pub parent_block_hash: B256,
    /// Payload attributes (Cancun version).
    pub payload_attributes: EthPayloadAttributes,
    /// Raw signed transactions to force-include in order.
    pub transactions: Vec<Bytes>,
    /// Optional extra data for the block header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_data: Option<Bytes>,
}

/// Testing RPC interface for building a block in a single call.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "testing"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "testing"))]
pub trait TestingApi {
    /// Builds a block using the provided parent, payload attributes, and transactions.
    ///
    /// See <https://github.com/marcindsobczak/execution-apis/blob/main/src/testing/testing_buildBlockV1.md>
    #[method(name = "buildBlockV1")]
    async fn build_block_v1(
        &self,
        request: TestingBuildBlockRequest,
    ) -> jsonrpsee::core::RpcResult<ExecutionPayloadEnvelopeV4>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::hex;
    use serde_json::json;
    use std::str::FromStr;

    #[test]
    fn request_example_roundtrip() {
        // Example request object (single element of params array in the spec).
        let raw = json!({
            "parentBlockHash": "0x1111111111111111111111111111111111111111111111111111111111111111",
            "payloadAttributes": {
                "timestamp": "0x6705D918",
                "prevRandao": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "suggestedFeeRecipient": "0x0000000000000000000000000000000000000000",
                "withdrawals": [],
                "parentBeaconBlockRoot": "0x2222222222222222222222222222222222222222222222222222222222222222"
            },
            "transactions": [
                "0x01",
                "0x02"
            ],
            "extraData": "0x746573745F6E616D65"
        });

        let req: TestingBuildBlockRequest = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(
            req.parent_block_hash,
            B256::from_str("0x1111111111111111111111111111111111111111111111111111111111111111").unwrap()
        );
        assert_eq!(req.transactions.len(), 2);
        let expected_extra = Bytes::from(hex!("746573745f6e616d65").to_vec());
        assert_eq!(req.extra_data, Some(expected_extra));

        // Round-trip keeps camelCase field names.
        let out = serde_json::to_value(req).unwrap();
        let obj = out.as_object().expect("object");
        assert!(obj.contains_key("parentBlockHash"));
        assert!(obj.contains_key("payloadAttributes"));
        assert!(obj.contains_key("transactions"));
        assert!(obj.contains_key("extraData"));
        assert_eq!(obj["transactions"].as_array().map(|arr| arr.len()), Some(2));
        assert_eq!(obj["parentBlockHash"], raw["parentBlockHash"]);
        assert_eq!(obj["payloadAttributes"]["parentBeaconBlockRoot"], raw["payloadAttributes"]["parentBeaconBlockRoot"]);
    }
}

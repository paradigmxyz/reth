//! `eth_` RPC API for pubsub subscription.

use alloy_json_rpc::RpcObject;
use jsonrpsee::{core::JsonRawValue, proc_macros::rpc};

/// Ethereum pub-sub rpc interface.
#[rpc(server, namespace = "eth")]
pub trait EthPubSubApi<T: RpcObject> {
    /// Create an ethereum subscription for the given params
    #[subscription(
        name = "subscribe" => "subscription",
        unsubscribe = "unsubscribe",
        item = alloy_rpc_types::pubsub::SubscriptionResult
    )]
    async fn subscribe(
        &self,
        kind: String,
        params: Option<Box<JsonRawValue>>,
    ) -> jsonrpsee::core::SubscriptionResult;
}

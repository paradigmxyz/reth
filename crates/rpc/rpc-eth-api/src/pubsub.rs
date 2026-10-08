//! `eth_` RPC API for pubsub subscription.

use alloy_json_rpc::RpcObject;
use alloy_rpc_types_eth::pubsub::{Params, SubscriptionKind};
use reth_jasonrpeesea::rpc;

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
        kind: SubscriptionKind,
        params: Option<Params>,
    ) -> reth_jasonrpeesea::SubscriptionResult;
}

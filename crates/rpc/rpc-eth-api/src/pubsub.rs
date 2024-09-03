//! `eth_` RPC API for pubsub subscription.

use alloy_json_rpc::RpcObject;
use jsonrpsee::proc_macros::rpc;
use reth_rpc_types::pubsub::{Params, SubscriptionKind};

/// Ethereum pub-sub rpc interface.
#[rpc(server, namespace = "eth")]
pub trait EthPubSubApi<T: RpcObject> {
    /// Create an ethereum subscription for the given params
    #[subscription(
        name = "subscribe" => "subscription",
        unsubscribe = "unsubscribe",
        item = reth_rpc_types::pubsub::SubscriptionResult
    )]
    async fn subscribe(
        &self,
        kind: SubscriptionKind,
        params: Option<Params>,
    ) -> jsonrpsee::core::SubscriptionResult;
}

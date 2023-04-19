use jsonrpsee::proc_macros::rpc;
use reth_rpc_types::pubsub::{Params, SubscriptionKind};

/// Ethereum pub-sub rpc interface.
#[rpc(server)]
pub trait EthPubSubApi {
    /// Create an ethereum subscription for the given params
    #[subscription(
        name = "eth_subscribe" => "eth_subscription",
        unsubscribe = "eth_unsubscribe",
        item = reth_rpc_types::pubsub::SubscriptionResult
    )]
    fn subscribe(&self, kind: SubscriptionKind, params: Option<Params>);
}

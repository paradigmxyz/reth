use jsonrpsee::proc_macros::rpc;
use reth_rpc_types::pubsub::{Kind, Params};

/// Ethereum pub-sub rpc interface.
#[rpc(server)]
pub trait EthPubSubApi {
    /// Create an ethereum subscription.
    #[subscription(
        name = "eth_subscribe",
        unsubscribe = "eth_unsubscribe",
        item = reth_rpc_types::pubsub::SubscriptionResult
    )]
    fn subscribe(&self, kind: Kind, params: Option<Params>);
}

use jsonrpsee::proc_macros::rpc;
use reth_rpc_types::pubsub::{Kind, Params};

/// Ethereum pub-sub rpc interface.
#[rpc(server)]
pub trait EthPubSubApi {
    /// Create an ethereum subscription.
    #[subscription(
        name = "eth_subscribe" => "eth_subscription",
        unsubscribe = "eth_unsubscribe",
        item = pubsub::Result
    )]
    fn subscribe(&self, kind: Kind, params: Option<Params>);
}

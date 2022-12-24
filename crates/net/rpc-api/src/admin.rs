use jsonrpsee::{core::RpcResult as Result, proc_macros::rpc};

/// Admin namespace rpc interface that gives access to several non-standard RPC methods.
#[rpc(server)]
#[async_trait::async_trait]
pub trait AdminApi {
    /// Adds the given node record to the peerset.
    #[method(name = "admin_addPeer")]
    async fn add_peer(&self, record: String) -> Result<bool>;

    /// Disconnects from a remote node if the connection exists.
    ///
    /// Returns true if the peer was successfully removed.
    #[method(name = "admin_removePeer")]
    async fn remove_peer(&self, record: String) -> Result<bool>;

    /// Creates an RPC subscription which serves events received from the network.
    #[subscription(
        name = "admin_peerEvents",
        unsubscribe = "admin_peerEvents_unsubscribe",
        item = String
    )]
    fn subscribe(&self);
}

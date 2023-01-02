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

    /// Adds the given node record to the trusted peerset.
    #[method(name = "admin_addTrustedPeer")]
    async fn add_trusted_peer(&self, record: String) -> Result<bool>;

    /// Removes a remote node from the trusted peer set, but it does not disconnect it
    /// automatically.
    ///
    /// Returns true if the peer was successfully removed.
    #[method(name = "admin_removeTrustedPeer")]
    async fn remove_trusted_peer(&self, record: String) -> Result<bool>;

    /// Creates an RPC subscription which serves events received from the network.
    #[subscription(
        name = "admin_peerEvents",
        unsubscribe = "admin_peerEvents_unsubscribe",
        item = String
    )]
    fn subscribe(&self);
}

use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_primitives::NodeRecord;
use reth_rpc_types::NodeInfo;

/// Admin namespace rpc interface that gives access to several non-standard RPC methods.
#[cfg_attr(not(feature = "client"), rpc(server))]
#[cfg_attr(feature = "client", rpc(server, client))]
#[async_trait::async_trait]
pub trait AdminApi {
    /// Adds the given node record to the peerset.
    #[method(name = "admin_addPeer")]
    fn add_peer(&self, record: NodeRecord) -> RpcResult<bool>;

    /// Disconnects from a remote node if the connection exists.
    ///
    /// Returns true if the peer was successfully removed.
    #[method(name = "admin_removePeer")]
    fn remove_peer(&self, record: NodeRecord) -> RpcResult<bool>;

    /// Adds the given node record to the trusted peerset.
    #[method(name = "admin_addTrustedPeer")]
    fn add_trusted_peer(&self, record: NodeRecord) -> RpcResult<bool>;

    /// Removes a remote node from the trusted peer set, but it does not disconnect it
    /// automatically.
    ///
    /// Returns true if the peer was successfully removed.
    #[method(name = "admin_removeTrustedPeer")]
    fn remove_trusted_peer(&self, record: NodeRecord) -> RpcResult<bool>;

    /// Creates an RPC subscription which serves events received from the network.
    #[subscription(
        name = "admin_peerEvents",
        unsubscribe = "admin_peerEvents_unsubscribe",
        item = String
    )]
    async fn subscribe_peer_events(&self) -> jsonrpsee::core::SubscriptionResult;

    /// Returns the ENR of the node.
    #[method(name = "admin_nodeInfo")]
    async fn node_info(&self) -> RpcResult<NodeInfo>;
}

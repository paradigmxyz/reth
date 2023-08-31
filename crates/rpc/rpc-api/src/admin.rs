use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_primitives::NodeRecord;
use reth_rpc_types::{NodeInfo, PeerInfo};

/// Admin namespace rpc interface that gives access to several non-standard RPC methods.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "admin"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "admin"))]
#[async_trait::async_trait]
pub trait AdminApi {
    /// Adds the given node record to the peerset.
    #[method(name = "addPeer")]
    fn add_peer(&self, record: NodeRecord) -> RpcResult<bool>;

    /// Disconnects from a remote node if the connection exists.
    ///
    /// Returns true if the peer was successfully removed.
    #[method(name = "removePeer")]
    fn remove_peer(&self, record: NodeRecord) -> RpcResult<bool>;

    /// Adds the given node record to the trusted peerset.
    #[method(name = "addTrustedPeer")]
    fn add_trusted_peer(&self, record: NodeRecord) -> RpcResult<bool>;

    /// Removes a remote node from the trusted peer set, but it does not disconnect it
    /// automatically.
    ///
    /// Returns true if the peer was successfully removed.
    #[method(name = "removeTrustedPeer")]
    fn remove_trusted_peer(&self, record: NodeRecord) -> RpcResult<bool>;

    /// The peers administrative property can be queried for all the information known about the
    /// connected remote nodes at the networking granularity. These include general information
    /// about the nodes themselves as participants of the devp2p P2P overlay protocol, as well as
    /// specialized information added by each of the running application protocols
    #[method(name = "peers")]
    async fn peers(&self) -> RpcResult<Vec<PeerInfo>>;

    /// Creates an RPC subscription which serves events received from the network.
    #[subscription(
        name = "peerEvents",
        unsubscribe = "peerEvents_unsubscribe",
        item = String
    )]
    async fn subscribe_peer_events(&self) -> jsonrpsee::core::SubscriptionResult;

    /// Returns the ENR of the node.
    #[method(name = "nodeInfo")]
    async fn node_info(&self) -> RpcResult<NodeInfo>;
}

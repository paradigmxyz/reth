use alloy_rpc_types_admin::{NodeInfo, PeerInfo};
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_network_peers::{AnyNode, NodeRecord};
use serde::{Deserialize, Serialize};

/// Request parameters for [`AdminApi::tracing_directives`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TracingDirectivesRequest {
    /// `RUST_LOG`-style tracing directives. Ignored when `ttlSecs` is `0`.
    #[serde(default)]
    pub directives: String,
    /// Number of seconds before the startup tracing configuration is restored.
    ///
    /// If omitted, the override remains active until explicitly reset. Set to `0` to reset
    /// immediately.
    pub ttl_secs: Option<u64>,
}

/// Result returned by [`AdminApi::tracing_directives`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TracingDirectivesResponse {
    /// The tracing directives that were applied.
    pub applied: String,
    /// Number of seconds before the override is reverted, or `None` for an indefinite override.
    pub ttl_secs: Option<u64>,
    /// The startup stdout directives that will be restored.
    ///
    /// Other reloadable layers restore their respective startup directives.
    pub reverts_to: String,
}

/// Admin namespace rpc interface that gives access to several non-standard RPC methods.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "admin"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "admin"))]
pub trait AdminApi {
    /// Adds the given node record to the peerset.
    #[method(name = "addPeer")]
    fn add_peer(&self, record: NodeRecord) -> RpcResult<bool>;

    /// Disconnects from a remote node if the connection exists.
    ///
    /// Returns true if the peer was successfully removed.
    #[method(name = "removePeer")]
    fn remove_peer(&self, record: AnyNode) -> RpcResult<bool>;

    /// Adds the given node record to the trusted peerset.
    #[method(name = "addTrustedPeer")]
    fn add_trusted_peer(&self, record: AnyNode) -> RpcResult<bool>;

    /// Removes a remote node from the trusted peer set, but it does not disconnect it
    /// automatically.
    ///
    /// Returns true if the peer was successfully removed.
    #[method(name = "removeTrustedPeer")]
    fn remove_trusted_peer(&self, record: AnyNode) -> RpcResult<bool>;

    /// Bans a remote node and disconnects an active non-trusted session if one exists.
    #[method(name = "banPeer")]
    fn ban_peer(&self, record: AnyNode) -> RpcResult<bool>;

    /// Unbans a remote node.
    #[method(name = "unbanPeer")]
    fn unban_peer(&self, record: AnyNode) -> RpcResult<bool>;

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

    /// Clears all transactions from the transaction pool.
    /// Returns the number of transactions that were removed from the pool.
    #[method(name = "clearTxpool")]
    async fn clear_txpool(&self) -> RpcResult<u64>;

    /// Temporarily overrides the node's tracing directives.
    ///
    /// The startup tracing configuration is restored after `ttlSecs`. If `ttlSecs` is omitted,
    /// the override remains active until reset by calling this method with `ttlSecs: 0`.
    #[method(name = "tracingDirectives")]
    async fn tracing_directives(
        &self,
        request: TracingDirectivesRequest,
    ) -> RpcResult<TracingDirectivesResponse>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_request_only_requires_ttl() {
        let request: TracingDirectivesRequest =
            serde_json::from_value(serde_json::json!({ "ttlSecs": 0 })).unwrap();
        assert!(request.directives.is_empty());
        assert_eq!(request.ttl_secs, Some(0));
    }
}

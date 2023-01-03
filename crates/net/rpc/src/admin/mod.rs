use jsonrpsee::core::RpcResult;
use reth_network::{peers::PeerKind, DisconnectReason, NetworkHandle};
use reth_primitives::NodeRecord;
use reth_rpc_api::AdminApiServer;

struct AdminApi {
    /// An interface to interact with the network
    network: NetworkHandle,
}

impl AdminApiServer for AdminApi {
    fn add_peer(&self, record: NodeRecord) -> RpcResult<bool> {
        self.network.add_peer(record.id, record.tcp_addr());
        Ok(true)
    }

    fn remove_peer(&self, record: NodeRecord) -> RpcResult<bool> {
        self.network.disconnect_peer_with_reason(record.id, DisconnectReason::DisconnectRequested);
        Ok(true)
    }

    fn add_trusted_peer(&self, record: NodeRecord) -> RpcResult<bool> {
        self.network.add_trusted_peer(record.id, record.tcp_addr());
        Ok(true)
    }

    fn remove_trusted_peer(&self, record: NodeRecord) -> RpcResult<bool> {
        self.network.add_peer_kind(record.id, PeerKind::Basic, record.tcp_addr());
        Ok(true)
    }

    fn subscribe(
        &self,
        _subscription_sink: jsonrpsee::SubscriptionSink,
    ) -> jsonrpsee::types::SubscriptionResult {
        todo!()
    }
}

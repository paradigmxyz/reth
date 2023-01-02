use jsonrpsee::core::RpcResult;
use reth_network::{peers::PeerKind, DisconnectReason, NetworkHandle};
use reth_primitives::NodeRecord;
use reth_rpc_api::AdminApiServer;

struct AdminApi {
    /// An interface to interact with the network
    network: NetworkHandle,
}

impl AdminApiServer for AdminApi {
    fn add_peer(&self, record: String) -> RpcResult<bool> {
        if let Ok(record) = record.parse::<NodeRecord>() {
            self.network.add_or_update_peer(record.id, PeerKind::Basic, record.tcp_addr());
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn remove_peer(&self, record: String) -> RpcResult<bool> {
        if let Ok(record) = record.parse::<NodeRecord>() {
            self.network
                .disconnect_peer_with_reason(record.id, DisconnectReason::DisconnectRequested);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn add_trusted_peer(&self, record: String) -> RpcResult<bool> {
        if let Ok(record) = record.parse::<NodeRecord>() {
            self.network.add_or_update_peer(record.id, PeerKind::Trusted, record.tcp_addr());
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn remove_trusted_peer(&self, record: String) -> RpcResult<bool> {
        if let Ok(record) = record.parse::<NodeRecord>() {
            self.network.add_or_update_peer(record.id, PeerKind::Basic, record.tcp_addr());
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn subscribe(
        &self,
        _subscription_sink: jsonrpsee::SubscriptionSink,
    ) -> jsonrpsee::types::SubscriptionResult {
        todo!()
    }
}

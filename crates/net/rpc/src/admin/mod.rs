use jsonrpsee::core::RpcResult;
use reth_network::{DisconnectReason, NetworkHandle};
use reth_primitives::NodeRecord;
use reth_rpc_api::AdminApiServer;

struct AdminApi {
    /// An interface to interact with the network
    network: NetworkHandle,
}

impl AdminApiServer for AdminApi {
    fn add_peer(&self, record: String) -> RpcResult<bool> {
        if let Ok(record) = record.parse::<NodeRecord>() {
            self.network.add_peer(record.id, record.tcp_addr());
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

    fn add_trusted_peer<'life0, 'async_trait>(
        &'life0 self,
        record: String,
    ) -> core::pin::Pin<
        Box<
            dyn core::future::Future<Output = jsonrpsee::core::RpcResult<bool>>
                + core::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        todo!()
    }

    fn remove_trusted_peer<'life0, 'async_trait>(
        &'life0 self,
        record: String,
    ) -> core::pin::Pin<
        Box<
            dyn core::future::Future<Output = jsonrpsee::core::RpcResult<bool>>
                + core::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        todo!()
    }

    fn subscribe(
        &self,
        subscription_sink: jsonrpsee::SubscriptionSink,
    ) -> jsonrpsee::types::SubscriptionResult {
        todo!()
    }
}

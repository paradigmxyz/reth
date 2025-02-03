use alloy_primitives::bytes::Bytes;
use alloy_provider::{Network, RootProvider};
use alloy_rpc_client::RpcClient;
use alloy_rpc_types_engine::JwtSecret;
use alloy_transport::utils::guess_local_url;
use alloy_transport_http::{
    hyper_util, hyper_util::rt::TokioExecutor, AuthLayer, Http, HyperClient,
};
use derive_more::Deref;
use http_body_util::Full;
use reqwest::Url;
use scroll_alloy_network::Scroll;

/// An authenticated [`alloy_provider::Provider`] to the [`super::ScrollEngineApi`].
#[derive(Debug, Clone, Deref)]
pub struct ScrollAuthEngineApiProvider<N: Network = Scroll> {
    auth_provider: RootProvider<N>,
}

impl ScrollAuthEngineApiProvider {
    /// Returns a new [`ScrollAuthEngineApiProvider`], authenticated for interfacing with the Engine
    /// API server at the provided URL using the passed JWT secret.
    pub fn new(jwt_secret: JwtSecret, url: Url) -> Self {
        let auth_layer = AuthLayer::new(jwt_secret);
        let hyper_client = hyper_util::client::legacy::Client::builder(TokioExecutor::new())
            .build_http::<Full<Bytes>>();

        let service = tower::ServiceBuilder::new().layer(auth_layer).service(hyper_client);
        let transport = HyperClient::<Full<Bytes>, _>::with_service(service);

        let is_url_local = guess_local_url(&url);
        let http = Http::with_client(transport, url);
        let client = RpcClient::new(http, is_url_local);

        let provider = RootProvider::new(client);
        Self { auth_provider: provider }
    }
}

#[cfg(all(test, feature = "scroll", not(feature = "optimism")))]
mod tests {
    use super::*;
    use crate::engine::ScrollEngineApi;
    use alloy_primitives::U64;
    use alloy_rpc_types_engine::{
        ClientCode, ClientVersionV1, ExecutionPayloadV1, ForkchoiceState, PayloadId,
    };
    use reth_engine_primitives::BeaconConsensusEngineHandle;
    use reth_payload_builder::{PayloadBuilderHandle, PayloadBuilderService};
    use reth_payload_primitives::PayloadTypes;
    use reth_primitives::{Block, TransactionSigned};
    use reth_primitives_traits::block::Block as _;
    use reth_provider::{test_utils::NoopProvider, CanonStateNotification};
    use reth_rpc_builder::auth::{AuthRpcModule, AuthServerConfig, AuthServerHandle};
    use reth_rpc_engine_api::{capabilities::EngineCapabilities, EngineApi};
    use reth_scroll_chainspec::SCROLL_MAINNET;
    use reth_scroll_engine_primitives::{
        ScrollBuiltPayload, ScrollEngineTypes, ScrollPayloadBuilderAttributes,
    };
    use reth_scroll_node::ScrollEngineValidator;
    use reth_scroll_payload::NoopPayloadJobGenerator;
    use reth_tasks::TokioTaskExecutor;
    use reth_transaction_pool::noop::NoopTransactionPool;
    use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
    use tokio::sync::mpsc::unbounded_channel;

    fn spawn_test_payload_service<T>() -> PayloadBuilderHandle<T>
    where
        T: PayloadTypes<
                PayloadBuilderAttributes = ScrollPayloadBuilderAttributes,
                BuiltPayload = ScrollBuiltPayload,
            > + 'static,
    {
        let (service, handle) = PayloadBuilderService::<
            NoopPayloadJobGenerator<ScrollPayloadBuilderAttributes, ScrollBuiltPayload>,
            futures_util::stream::Empty<CanonStateNotification>,
            T,
        >::new(Default::default(), futures_util::stream::empty());
        tokio::spawn(service);
        handle
    }

    async fn launch_auth(jwt_secret: JwtSecret) -> AuthServerHandle {
        let config = AuthServerConfig::builder(jwt_secret)
            .socket_addr(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)))
            .build();
        let (tx, _rx) = unbounded_channel();
        let beacon_engine_handle = BeaconConsensusEngineHandle::<ScrollEngineTypes>::new(tx);
        let client = ClientVersionV1 {
            code: ClientCode::RH,
            name: "Reth".to_string(),
            version: "v0.2.0-beta.5".to_string(),
            commit: "defa64b2".to_string(),
        };

        let engine_api = EngineApi::new(
            NoopProvider::default(),
            SCROLL_MAINNET.clone(),
            beacon_engine_handle,
            spawn_test_payload_service().into(),
            NoopTransactionPool::default(),
            Box::<TokioTaskExecutor>::default(),
            client,
            EngineCapabilities::default(),
            ScrollEngineValidator::new(SCROLL_MAINNET.clone()),
        );
        let module = AuthRpcModule::new(engine_api);
        module.start_server(config).await.unwrap()
    }

    #[allow(unused_must_use)]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_engine_api_provider() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();

        let secret = JwtSecret::random();
        let handle = launch_auth(secret).await;
        let url = handle.http_url().parse()?;
        let provider = ScrollAuthEngineApiProvider::new(secret, url);

        let block = Block::<TransactionSigned>::default().seal_slow();
        let execution_payload =
            ExecutionPayloadV1::from_block_unchecked(block.hash(), &block.clone().into_block());
        provider.new_payload_v1(execution_payload).await;
        provider.fork_choice_updated_v1(ForkchoiceState::default(), None).await;
        provider.get_payload_v1(PayloadId::new([0, 0, 0, 0, 0, 0, 0, 0])).await;
        provider.get_payload_bodies_by_hash_v1(vec![]).await;
        provider.get_payload_bodies_by_range_v1(U64::ZERO, U64::from(1u64)).await;
        provider.exchange_capabilities(vec![]).await;

        Ok(())
    }
}

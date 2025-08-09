
use alloy_primitives::{BlockHash, B256, U64};
use alloy_rpc_types_engine::{
    ClientVersionV1, ExecutionPayloadBodiesV1, ExecutionPayloadInputV2, ExecutionPayloadV3,
    ForkchoiceState, ForkchoiceUpdated, PayloadId, PayloadStatus,
};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee_core::{server::RpcModule, RpcResult};
use reth_chainspec::EthereumHardforks;
use reth_engine_primitives::{EngineApiValidator, EngineTypes};
use reth_rpc_api::IntoEngineApiRpcModule;
use reth_rpc_engine_api::EngineApi;
use reth_storage_api::{BlockReader, HeaderProvider, StateProviderFactory};
use reth_transaction_pool::TransactionPool;
use tracing::trace;

use reth_arbitrum_payload::ArbExecutionData;

/// Supported Engine capabilities over the engine endpoint for Arbitrum.
pub const ARB_ENGINE_CAPABILITIES: &[&str] = &[
    "engine_forkchoiceUpdatedV1",
    "engine_forkchoiceUpdatedV2",
    "engine_forkchoiceUpdatedV3",
    "engine_getClientVersionV1",
    "engine_getPayloadV2",
    "engine_getPayloadV3",
    "engine_newPayloadV2",
    "engine_newPayloadV3",
    "engine_getPayloadBodiesByHashV1",
    "engine_getPayloadBodiesByRangeV1",
];

#[cfg_attr(not(feature = "client"), rpc(server, namespace = "engine"), server_bounds(Engine::PayloadAttributes: jsonrpsee::core::DeserializeOwned))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "engine", client_bounds(Engine::PayloadAttributes: jsonrpsee::core::Serialize + Clone), server_bounds(Engine::PayloadAttributes: jsonrpsee::core::DeserializeOwned)))]
pub trait ArbEngineApi<Engine: EngineTypes> {
    #[method(name = "newPayloadV2")]
    async fn new_payload_v2(&self, payload: ExecutionPayloadInputV2) -> RpcResult<PayloadStatus>;

    #[method(name = "newPayloadV3")]
    async fn new_payload_v3(
        &self,
        payload: ExecutionPayloadV3,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
    ) -> RpcResult<PayloadStatus>;


    #[method(name = "forkchoiceUpdatedV1")]
    async fn fork_choice_updated_v1(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<Engine::PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated>;

    #[method(name = "forkchoiceUpdatedV2")]
    async fn fork_choice_updated_v2(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<Engine::PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated>;

    #[method(name = "forkchoiceUpdatedV3")]
    async fn fork_choice_updated_v3(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<Engine::PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated>;

    #[method(name = "getPayloadV2")]
    async fn get_payload_v2(&self, payload_id: PayloadId)
        -> RpcResult<Engine::ExecutionPayloadEnvelopeV2>;

    #[method(name = "getPayloadV3")]
    async fn get_payload_v3(&self, payload_id: PayloadId)
        -> RpcResult<Engine::ExecutionPayloadEnvelopeV3>;


    #[method(name = "getPayloadBodiesByHashV1")]
    async fn get_payload_bodies_by_hash_v1(
        &self,
        block_hashes: Vec<BlockHash>,
    ) -> RpcResult<ExecutionPayloadBodiesV1>;

    #[method(name = "getPayloadBodiesByRangeV1")]
    async fn get_payload_bodies_by_range_v1(
        &self,
        start: U64,
        count: U64,
    ) -> RpcResult<ExecutionPayloadBodiesV1>;

    #[method(name = "getClientVersionV1")]
    async fn get_client_version_v1(
        &self,
        client_version: ClientVersionV1,
    ) -> RpcResult<Vec<ClientVersionV1>>;

    #[method(name = "exchangeCapabilities")]
    async fn exchange_capabilities(&self, capabilities: Vec<String>) -> RpcResult<Vec<String>>;
}

#[derive(Debug)]
pub struct ArbEngineApi<Provider, EngineT: EngineTypes, Pool, Validator, ChainSpec> {
    inner: EngineApi<Provider, EngineT, Pool, Validator, ChainSpec>,
}

impl<Provider, PayloadT, Pool, Validator, ChainSpec> Clone
    for ArbEngineApi<Provider, PayloadT, Pool, Validator, ChainSpec>
where
    PayloadT: EngineTypes,
{
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<Provider, PayloadT, Pool, Validator, ChainSpec>
    ArbEngineApi<Provider, PayloadT, Pool, Validator, ChainSpec>
where
    Provider: HeaderProvider + BlockReader + StateProviderFactory + 'static,
    PayloadT: EngineTypes,
    Pool: TransactionPool + 'static,
    Validator: EngineApiValidator<PayloadT>,
    ChainSpec: EthereumHardforks + Send + Sync + 'static,
{
    pub fn new(inner: EngineApi<Provider, PayloadT, Pool, Validator, ChainSpec>) -> Self {
        Self { inner }
    }
}

#[async_trait::async_trait]
impl<Provider, EngineT, Pool, Validator, ChainSpec> ArbEngineApiServer<EngineT>
    for ArbEngineApi<Provider, EngineT, Pool, Validator, ChainSpec>
where
    Provider: HeaderProvider + BlockReader + StateProviderFactory + 'static,
    EngineT: EngineTypes<ExecutionData = ArbExecutionData>,
    EngineT::PayloadAttributes: jsonrpsee_core::DeserializeOwned,
    Pool: TransactionPool + 'static,
    Validator: EngineApiValidator<EngineT>,
    ChainSpec: EthereumHardforks + Send + Sync + 'static,
{
    async fn new_payload_v2(&self, payload: ExecutionPayloadInputV2) -> RpcResult<PayloadStatus> {
        trace!(target: "rpc::engine", "Serving engine_newPayloadV2");
        let payload = ArbExecutionData::v2(payload);
        Ok(self.inner.new_payload_v2_metered(payload).await?)
    }

    async fn new_payload_v3(
        &self,
        payload: ExecutionPayloadV3,
        _versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
    ) -> RpcResult<PayloadStatus> {
        trace!(target: "rpc::engine", "Serving engine_newPayloadV3");
        let payload = ArbExecutionData::v3(payload, parent_beacon_block_root);
        Ok(self.inner.new_payload_v3_metered(payload).await?)
    }


    async fn fork_choice_updated_v1(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<EngineT::PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        Ok(self.inner.fork_choice_updated_v1_metered(fork_choice_state, payload_attributes).await?)
    }

    async fn fork_choice_updated_v2(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<EngineT::PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        Ok(self.inner.fork_choice_updated_v2_metered(fork_choice_state, payload_attributes).await?)
    }

    async fn fork_choice_updated_v3(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<EngineT::PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        Ok(self.inner.fork_choice_updated_v3_metered(fork_choice_state, payload_attributes).await?)
    }

    async fn get_payload_v2(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<EngineT::ExecutionPayloadEnvelopeV2> {
        Ok(self.inner.get_payload_v2_metered(payload_id).await?)
    }

    async fn get_payload_v3(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<EngineT::ExecutionPayloadEnvelopeV3> {
        Ok(self.inner.get_payload_v3_metered(payload_id).await?)
    }


    async fn get_payload_bodies_by_hash_v1(
        &self,
        block_hashes: Vec<BlockHash>,
    ) -> RpcResult<ExecutionPayloadBodiesV1> {
        Ok(self.inner.get_payload_bodies_by_hash_v1_metered(block_hashes).await?)
    }

    async fn get_payload_bodies_by_range_v1(
        &self,
        start: U64,
        count: U64,
    ) -> RpcResult<ExecutionPayloadBodiesV1> {
        Ok(self.inner.get_payload_bodies_by_range_v1_metered(start.to(), count.to()).await?)
    }

    async fn get_client_version_v1(
        &self,
        client: ClientVersionV1,
    ) -> RpcResult<Vec<ClientVersionV1>> {
        Ok(self.inner.get_client_version_v1(client)?)
    }

    async fn exchange_capabilities(
        &self,
        _capabilities: Vec<String>,
    ) -> RpcResult<Vec<String>> {
        Ok(self.inner.capabilities().list())
    }
}

impl<Provider, EngineT, Pool, Validator, ChainSpec> IntoEngineApiRpcModule
    for ArbEngineApi<Provider, EngineT, Pool, Validator, ChainSpec>
where
    EngineT: EngineTypes,
    Self: ArbEngineApiServer<EngineT>,
{
    fn into_rpc_module(self) -> RpcModule<()> {
        self.into_rpc().remove_context()
    }
}

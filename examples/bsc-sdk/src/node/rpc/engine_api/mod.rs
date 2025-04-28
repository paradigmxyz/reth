use alloy_eips::{
    eip4844::{BlobAndProofV1, BlobAndProofV2},
    eip7685::RequestsOrHash,
};
use alloy_primitives::{BlockHash, B256, U64};
use alloy_rpc_types_engine::{
    ClientVersionV1, ExecutionPayloadBodiesV1, ForkchoiceState, ForkchoiceUpdated, PayloadId,
    PayloadStatus,
};
use derive_more::Constructor;
use jsonrpsee::proc_macros::rpc;
use jsonrpsee_core::{server::RpcModule, RpcResult};
use reth::{
    rpc::{api::IntoEngineApiRpcModule, result::internal_rpc_err},
    transaction_pool::TransactionPool,
};
use reth_chainspec::EthereumHardforks;
use reth_engine_primitives::{EngineTypes, EngineValidator};
use reth_provider::{BlockReader, HeaderProvider, StateProviderFactory};
use reth_rpc_engine_api::EngineApi;
use validator::BscExecutionData;

pub mod builder;
pub mod payload;
pub mod validator;

#[cfg_attr(not(feature = "client"), rpc(server, namespace = "engine"), server_bounds(Engine::PayloadAttributes: jsonrpsee::core::DeserializeOwned))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "engine", client_bounds(Engine::PayloadAttributes: jsonrpsee::core::Serialize + Clone), server_bounds(Engine::PayloadAttributes: jsonrpsee::core::DeserializeOwned)))]
pub trait BscEngineApi<Engine: EngineTypes> {
    /// See also <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/paris.md#engine_newpayloadv1>
    /// Caution: This should not accept the `withdrawals` field
    #[method(name = "newPayloadV1")]
    async fn new_payload_v1(&self, payload: BscExecutionData) -> RpcResult<PayloadStatus>;

    /// See also <https://github.com/ethereum/execution-apis/blob/584905270d8ad665718058060267061ecfd79ca5/src/engine/shanghai.md#engine_newpayloadv2>
    #[method(name = "newPayloadV2")]
    async fn new_payload_v2(&self, payload: BscExecutionData) -> RpcResult<PayloadStatus>;

    /// Post Cancun payload handler
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/cancun.md#engine_newpayloadv3>
    #[method(name = "newPayloadV3")]
    async fn new_payload_v3(
        &self,
        payload: BscExecutionData,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
    ) -> RpcResult<PayloadStatus>;

    /// Post Prague payload handler
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/prague.md#engine_newpayloadv4>
    #[method(name = "newPayloadV4")]
    async fn new_payload_v4(
        &self,
        payload: BscExecutionData,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
        execution_requests: RequestsOrHash,
    ) -> RpcResult<PayloadStatus>;

    /// See also <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/paris.md#engine_forkchoiceupdatedv1>
    ///
    /// Caution: This should not accept the `withdrawals` field in the payload attributes.
    #[method(name = "forkchoiceUpdatedV1")]
    async fn fork_choice_updated_v1(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<Engine::PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated>;

    /// Post Shanghai forkchoice update handler
    ///
    /// This is the same as `forkchoiceUpdatedV1`, but expects an additional `withdrawals` field in
    /// the `payloadAttributes`, if payload attributes are provided.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/shanghai.md#engine_forkchoiceupdatedv2>
    ///
    /// Caution: This should not accept the `parentBeaconBlockRoot` field in the payload
    /// attributes.
    #[method(name = "forkchoiceUpdatedV2")]
    async fn fork_choice_updated_v2(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<Engine::PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated>;

    /// Post Cancun forkchoice update handler
    ///
    /// This is the same as `forkchoiceUpdatedV2`, but expects an additional
    /// `parentBeaconBlockRoot` field in the `payloadAttributes`, if payload attributes
    /// are provided.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/cancun.md#engine_forkchoiceupdatedv3>
    #[method(name = "forkchoiceUpdatedV3")]
    async fn fork_choice_updated_v3(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<Engine::PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated>;

    /// See also <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/paris.md#engine_getpayloadv1>
    ///
    /// Returns the most recent version of the payload that is available in the corresponding
    /// payload build process at the time of receiving this call.
    ///
    /// Caution: This should not return the `withdrawals` field
    ///
    /// Note:
    /// > Provider software MAY stop the corresponding build process after serving this call.
    #[method(name = "getPayloadV1")]
    async fn get_payload_v1(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<Engine::ExecutionPayloadEnvelopeV1>;

    /// See also <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/shanghai.md#engine_getpayloadv2>
    ///
    /// Returns the most recent version of the payload that is available in the corresponding
    /// payload build process at the time of receiving this call. Note:
    /// > Provider software MAY stop the corresponding build process after serving this call.
    #[method(name = "getPayloadV2")]
    async fn get_payload_v2(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<Engine::ExecutionPayloadEnvelopeV2>;

    /// Post Cancun payload handler which also returns a blobs bundle.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/cancun.md#engine_getpayloadv3>
    ///
    /// Returns the most recent version of the payload that is available in the corresponding
    /// payload build process at the time of receiving this call. Note:
    /// > Provider software MAY stop the corresponding build process after serving this call.
    #[method(name = "getPayloadV3")]
    async fn get_payload_v3(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<Engine::ExecutionPayloadEnvelopeV3>;

    /// Post Prague payload handler.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/prague.md#engine_getpayloadv4>
    ///
    /// Returns the most recent version of the payload that is available in the corresponding
    /// payload build process at the time of receiving this call. Note:
    /// > Provider software MAY stop the corresponding build process after serving this call.
    #[method(name = "getPayloadV4")]
    async fn get_payload_v4(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<Engine::ExecutionPayloadEnvelopeV4>;

    /// See also <https://github.com/ethereum/execution-apis/blob/6452a6b194d7db269bf1dbd087a267251d3cc7f8/src/engine/shanghai.md#engine_getpayloadbodiesbyhashv1>
    #[method(name = "getPayloadBodiesByHashV1")]
    async fn get_payload_bodies_by_hash_v1(
        &self,
        block_hashes: Vec<BlockHash>,
    ) -> RpcResult<ExecutionPayloadBodiesV1>;

    /// See also <https://github.com/ethereum/execution-apis/blob/6452a6b194d7db269bf1dbd087a267251d3cc7f8/src/engine/shanghai.md#engine_getpayloadbodiesbyrangev1>
    ///
    /// Returns the execution payload bodies by the range starting at `start`, containing `count`
    /// blocks.
    ///
    /// WARNING: This method is associated with the BeaconBlocksByRange message in the consensus
    /// layer p2p specification, meaning the input should be treated as untrusted or potentially
    /// adversarial.
    ///
    /// Implementers should take care when acting on the input to this method, specifically
    /// ensuring that the range is limited properly, and that the range boundaries are computed
    /// correctly and without panics.
    #[method(name = "getPayloadBodiesByRangeV1")]
    async fn get_payload_bodies_by_range_v1(
        &self,
        start: U64,
        count: U64,
    ) -> RpcResult<ExecutionPayloadBodiesV1>;

    /// This function will return the ClientVersionV1 object.
    /// See also:
    /// <https://github.com/ethereum/execution-apis/blob/03911ffc053b8b806123f1fc237184b0092a485a/src/engine/identification.md#engine_getclientversionv1>make fmt
    ///
    ///
    /// - When connected to a single execution client, the consensus client **MUST** receive an
    ///   array with a single `ClientVersionV1` object.
    /// - When connected to multiple execution clients via a multiplexer, the multiplexer **MUST**
    ///   concatenate the responses from each execution client into a single,
    /// flat array before returning the response to the consensus client.
    #[method(name = "getClientVersionV1")]
    async fn get_client_version_v1(
        &self,
        client_version: ClientVersionV1,
    ) -> RpcResult<Vec<ClientVersionV1>>;

    /// See also <https://github.com/ethereum/execution-apis/blob/6452a6b194d7db269bf1dbd087a267251d3cc7f8/src/engine/common.md#capabilities>
    #[method(name = "exchangeCapabilities")]
    async fn exchange_capabilities(&self, capabilities: Vec<String>) -> RpcResult<Vec<String>>;

    /// Fetch blobs for the consensus layer from the in-memory blob cache.
    #[method(name = "getBlobsV1")]
    async fn get_blobs_v1(
        &self,
        versioned_hashes: Vec<B256>,
    ) -> RpcResult<Vec<Option<BlobAndProofV1>>>;

    /// Fetch blobs for the consensus layer from the blob store.
    #[method(name = "getBlobsV2")]
    async fn get_blobs_v2(
        &self,
        versioned_hashes: Vec<B256>,
    ) -> RpcResult<Vec<Option<BlobAndProofV2>>>;
}

impl<Provider, EngineT, Pool, Validator, ChainSpec> IntoEngineApiRpcModule
    for BscEngineApi<Provider, EngineT, Pool, Validator, ChainSpec>
where
    EngineT: EngineTypes,
    Self: BscEngineApiServer<EngineT>,
{
    fn into_rpc_module(self) -> RpcModule<()> {
        self.into_rpc().remove_context()
    }
}

#[derive(Debug, Constructor)]
pub struct BscEngineApi<Provider, EngineT: EngineTypes, Pool, Validator, ChainSpec> {
    inner: EngineApi<Provider, EngineT, Pool, Validator, ChainSpec>,
}

#[async_trait::async_trait]
impl<Provider, EngineT, Pool, Validator, ChainSpec> BscEngineApiServer<EngineT>
    for BscEngineApi<Provider, EngineT, Pool, Validator, ChainSpec>
where
    Provider: HeaderProvider + BlockReader + StateProviderFactory + 'static,
    EngineT: EngineTypes<ExecutionData = BscExecutionData>,
    Pool: TransactionPool + 'static,
    Validator: EngineValidator<EngineT>,
    ChainSpec: EthereumHardforks + Send + Sync + 'static,
{
    async fn new_payload_v1(&self, payload: BscExecutionData) -> RpcResult<PayloadStatus> {
        Ok(self.inner.new_payload_v1(payload).await?)
    }

    async fn new_payload_v2(&self, payload: BscExecutionData) -> RpcResult<PayloadStatus> {
        Ok(self.inner.new_payload_v2(payload).await?)
    }

    async fn new_payload_v3(
        &self,
        payload: BscExecutionData,
        _versioned_hashes: Vec<B256>,
        _parent_beacon_block_root: B256,
    ) -> RpcResult<PayloadStatus> {
        Ok(self.inner.new_payload_v3(payload).await?)
    }

    async fn new_payload_v4(
        &self,
        payload: BscExecutionData,
        _versioned_hashes: Vec<B256>,
        _parent_beacon_block_root: B256,
        _execution_requests: RequestsOrHash,
    ) -> RpcResult<PayloadStatus> {
        Ok(self.inner.new_payload_v4(payload).await?)
    }

    async fn get_payload_v4(
        &self,
        _payload_id: PayloadId,
    ) -> RpcResult<EngineT::ExecutionPayloadEnvelopeV4> {
        //Ok(self.inner.get_payload_v4(payload_id).await?)
        todo!()
    }

    async fn fork_choice_updated_v1(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<EngineT::PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        Ok(self.inner.fork_choice_updated_v1(fork_choice_state, payload_attributes).await?)
    }

    async fn fork_choice_updated_v2(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<EngineT::PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        Ok(self.inner.fork_choice_updated_v2(fork_choice_state, payload_attributes).await?)
    }

    async fn fork_choice_updated_v3(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<EngineT::PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        Ok(self.inner.fork_choice_updated_v3(fork_choice_state, payload_attributes).await?)
    }

    async fn get_payload_v1(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<EngineT::ExecutionPayloadEnvelopeV1> {
        Ok(self.inner.get_payload_v1(payload_id).await?)
    }

    async fn get_payload_v2(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<EngineT::ExecutionPayloadEnvelopeV2> {
        Ok(self.inner.get_payload_v2(payload_id).await?)
    }

    async fn get_payload_v3(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<EngineT::ExecutionPayloadEnvelopeV3> {
        Ok(self.inner.get_payload_v3(payload_id).await?)
    }

    async fn get_payload_bodies_by_hash_v1(
        &self,
        block_hashes: Vec<BlockHash>,
    ) -> RpcResult<ExecutionPayloadBodiesV1> {
        Ok(self.inner.get_payload_bodies_by_hash_v1(block_hashes).await?)
    }

    async fn get_payload_bodies_by_range_v1(
        &self,
        start: U64,
        count: U64,
    ) -> RpcResult<ExecutionPayloadBodiesV1> {
        Ok(self.inner.get_payload_bodies_by_range_v1(start.to(), count.to()).await?)
    }

    async fn get_client_version_v1(
        &self,
        client_version: ClientVersionV1,
    ) -> RpcResult<Vec<ClientVersionV1>> {
        Ok(self.inner.get_client_version_v1(client_version)?)
    }

    async fn exchange_capabilities(&self, _capabilities: Vec<String>) -> RpcResult<Vec<String>> {
        Ok(self.inner.capabilities().list())
    }

    async fn get_blobs_v1(
        &self,
        _versioned_hashes: Vec<B256>,
    ) -> RpcResult<Vec<Option<BlobAndProofV1>>> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn get_blobs_v2(
        &self,
        _versioned_hashes: Vec<B256>,
    ) -> RpcResult<Vec<Option<BlobAndProofV2>>> {
        Err(internal_rpc_err("unimplemented"))
    }
}

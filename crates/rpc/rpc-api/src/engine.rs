//! Server traits for the engine API
//!
//! This contains the `engine_` namespace and the subset of the `eth_` namespace that is exposed to
//! the consensus client.

use alloy_eips::{eip4844::BlobAndProofV1, BlockId, BlockNumberOrTag};
use alloy_json_rpc::RpcObject;
use alloy_primitives::{Address, BlockHash, Bytes, B256, U256, U64};
use alloy_rpc_types::{
    state::StateOverride, BlockOverrides, EIP1186AccountProofResponse, Filter, Log, SyncStatus,
};
use alloy_rpc_types_engine::{
    ClientVersionV1, ExecutionPayloadBodiesV1, ExecutionPayloadBodiesV2, ExecutionPayloadInputV2,
    ExecutionPayloadV1, ExecutionPayloadV3, ExecutionPayloadV4, ForkchoiceState, ForkchoiceUpdated,
    PayloadId, PayloadStatus, TransitionConfiguration,
};
use alloy_rpc_types_eth::transaction::TransactionRequest;
use alloy_serde::JsonStorageKey;
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_engine_primitives::EngineTypes;
// NOTE: We can't use associated types in the `EngineApi` trait because of jsonrpsee, so we use a
// generic here. It would be nice if the rpc macro would understand which types need to have serde.
// By default, if the trait has a generic, the rpc macro will add e.g. `Engine: DeserializeOwned` to
// the trait bounds, which is not what we want, because `Types` is not used directly in any of the
// trait methods. Instead, we have to add the bounds manually. This would be disastrous if we had
// more than one associated type used in the trait methods.

#[cfg_attr(not(feature = "client"), rpc(server, namespace = "engine"), server_bounds(Engine::PayloadAttributes: jsonrpsee::core::DeserializeOwned))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "engine", client_bounds(Engine::PayloadAttributes: jsonrpsee::core::Serialize + Clone), server_bounds(Engine::PayloadAttributes: jsonrpsee::core::DeserializeOwned)))]
pub trait EngineApi<Engine: EngineTypes> {
    /// See also <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/paris.md#engine_newpayloadv1>
    /// Caution: This should not accept the `withdrawals` field
    #[method(name = "newPayloadV1")]
    async fn new_payload_v1(&self, payload: ExecutionPayloadV1) -> RpcResult<PayloadStatus>;

    /// See also <https://github.com/ethereum/execution-apis/blob/584905270d8ad665718058060267061ecfd79ca5/src/engine/shanghai.md#engine_newpayloadv2>
    #[method(name = "newPayloadV2")]
    async fn new_payload_v2(&self, payload: ExecutionPayloadInputV2) -> RpcResult<PayloadStatus>;

    /// Post Cancun payload handler
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/cancun.md#engine_newpayloadv3>
    #[method(name = "newPayloadV3")]
    async fn new_payload_v3(
        &self,
        payload: ExecutionPayloadV3,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
    ) -> RpcResult<PayloadStatus>;

    /// Post Prague payload handler
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/prague.md#engine_newpayloadv4>
    #[method(name = "newPayloadV4")]
    async fn new_payload_v4(
        &self,
        payload: ExecutionPayloadV4,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
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
    async fn get_payload_v1(&self, payload_id: PayloadId) -> RpcResult<Engine::ExecutionPayloadV1>;

    /// See also <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/shanghai.md#engine_getpayloadv2>
    ///
    /// Returns the most recent version of the payload that is available in the corresponding
    /// payload build process at the time of receiving this call. Note:
    /// > Provider software MAY stop the corresponding build process after serving this call.
    #[method(name = "getPayloadV2")]
    async fn get_payload_v2(&self, payload_id: PayloadId) -> RpcResult<Engine::ExecutionPayloadV2>;

    /// Post Cancun payload handler which also returns a blobs bundle.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/cancun.md#engine_getpayloadv3>
    ///
    /// Returns the most recent version of the payload that is available in the corresponding
    /// payload build process at the time of receiving this call. Note:
    /// > Provider software MAY stop the corresponding build process after serving this call.
    #[method(name = "getPayloadV3")]
    async fn get_payload_v3(&self, payload_id: PayloadId) -> RpcResult<Engine::ExecutionPayloadV3>;

    /// Post Prague payload handler.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/prague.md#engine_getpayloadv4>
    ///
    /// Returns the most recent version of the payload that is available in the corresponding
    /// payload build process at the time of receiving this call. Note:
    /// > Provider software MAY stop the corresponding build process after serving this call.
    #[method(name = "getPayloadV4")]
    async fn get_payload_v4(&self, payload_id: PayloadId) -> RpcResult<Engine::ExecutionPayloadV4>;

    /// See also <https://github.com/ethereum/execution-apis/blob/6452a6b194d7db269bf1dbd087a267251d3cc7f8/src/engine/shanghai.md#engine_getpayloadbodiesbyhashv1>
    #[method(name = "getPayloadBodiesByHashV1")]
    async fn get_payload_bodies_by_hash_v1(
        &self,
        block_hashes: Vec<BlockHash>,
    ) -> RpcResult<ExecutionPayloadBodiesV1>;

    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/prague.md#engine_getpayloadbodiesbyhashv2>
    #[method(name = "getPayloadBodiesByHashV2")]
    async fn get_payload_bodies_by_hash_v2(
        &self,
        block_hashes: Vec<BlockHash>,
    ) -> RpcResult<ExecutionPayloadBodiesV2>;

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

    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/prague.md#engine_getpayloadbodiesbyrangev2>
    ///
    /// Similar to `getPayloadBodiesByRangeV1`, but returns [`ExecutionPayloadBodiesV2`]
    #[method(name = "getPayloadBodiesByRangeV2")]
    async fn get_payload_bodies_by_range_v2(
        &self,
        start: U64,
        count: U64,
    ) -> RpcResult<ExecutionPayloadBodiesV2>;

    /// See also <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/paris.md#engine_exchangetransitionconfigurationv1>
    ///
    /// Note: This method will be deprecated after the cancun hardfork:
    ///
    /// > Consensus and execution layer clients MAY remove support of this method after Cancun. If
    /// > no longer supported, this method MUST be removed from the engine_exchangeCapabilities
    /// > request or response list depending on whether it is consensus or execution layer client.
    #[method(name = "exchangeTransitionConfigurationV1")]
    async fn exchange_transition_configuration(
        &self,
        transition_configuration: TransitionConfiguration,
    ) -> RpcResult<TransitionConfiguration>;

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
}

/// A subset of the ETH rpc interface: <https://ethereum.github.io/execution-apis/api-documentation/>
///
/// Specifically for the engine auth server: <https://github.com/ethereum/execution-apis/blob/main/src/engine/common.md#underlying-protocol>
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "eth"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "eth"))]
pub trait EngineEthApi<B: RpcObject> {
    /// Returns an object with data about the sync status or false.
    #[method(name = "syncing")]
    fn syncing(&self) -> RpcResult<SyncStatus>;

    /// Returns the chain ID of the current network.
    #[method(name = "chainId")]
    async fn chain_id(&self) -> RpcResult<Option<U64>>;

    /// Returns the number of most recent block.
    #[method(name = "blockNumber")]
    fn block_number(&self) -> RpcResult<U256>;

    /// Executes a new message call immediately without creating a transaction on the block chain.
    #[method(name = "call")]
    async fn call(
        &self,
        request: TransactionRequest,
        block_id: Option<BlockId>,
        state_overrides: Option<StateOverride>,
        block_overrides: Option<Box<BlockOverrides>>,
    ) -> RpcResult<Bytes>;

    /// Returns code at a given address at given block number.
    #[method(name = "getCode")]
    async fn get_code(&self, address: Address, block_id: Option<BlockId>) -> RpcResult<Bytes>;

    /// Returns information about a block by hash.
    #[method(name = "getBlockByHash")]
    async fn block_by_hash(&self, hash: B256, full: bool) -> RpcResult<Option<B>>;

    /// Returns information about a block by number.
    #[method(name = "getBlockByNumber")]
    async fn block_by_number(&self, number: BlockNumberOrTag, full: bool) -> RpcResult<Option<B>>;

    /// Sends signed transaction, returning its hash.
    #[method(name = "sendRawTransaction")]
    async fn send_raw_transaction(&self, bytes: Bytes) -> RpcResult<B256>;

    /// Returns logs matching given filter object.
    #[method(name = "getLogs")]
    async fn logs(&self, filter: Filter) -> RpcResult<Vec<Log>>;

    /// Returns the account and storage values of the specified account including the Merkle-proof.
    /// This call can be used to verify that the data you are pulling from is not tampered with.
    #[method(name = "getProof")]
    async fn get_proof(
        &self,
        address: Address,
        keys: Vec<JsonStorageKey>,
        block_number: Option<BlockId>,
    ) -> RpcResult<EIP1186AccountProofResponse>;
}

use crate::{
    capabilities::EngineCapabilities, metrics::EngineApiMetrics, EngineApiError, EngineApiResult,
};
use alloy_eips::eip4844::BlobAndProofV1;
use alloy_primitives::{BlockHash, BlockNumber, B256, U64};
use alloy_rpc_types_engine::{
    CancunPayloadFields, ClientVersionV1, ExecutionPayload, ExecutionPayloadBodiesV1,
    ExecutionPayloadBodiesV2, ExecutionPayloadInputV2, ExecutionPayloadV1, ExecutionPayloadV3,
    ExecutionPayloadV4, ForkchoiceState, ForkchoiceUpdated, PayloadId, PayloadStatus,
    TransitionConfiguration,
};
use async_trait::async_trait;
use jsonrpsee_core::RpcResult;
use reth_beacon_consensus::BeaconConsensusEngineHandle;
use reth_chainspec::{EthereumHardforks, Hardforks};
use reth_engine_primitives::{EngineTypes, EngineValidator};
use reth_evm::provider::EvmEnvProvider;
use reth_payload_builder::PayloadStore;
use reth_payload_primitives::{
    validate_payload_timestamp, EngineApiMessageVersion, PayloadBuilderAttributes,
    PayloadOrAttributes,
};
use reth_primitives::{Block, BlockHashOrNumber, EthereumHardfork};
use reth_rpc_api::EngineApiServer;
use reth_rpc_types_compat::engine::payload::{
    convert_payload_input_v2_to_payload, convert_to_payload_body_v1, convert_to_payload_body_v2,
};
use reth_storage_api::{BlockReader, HeaderProvider, StateProviderFactory};
use reth_tasks::TaskSpawner;
use reth_transaction_pool::TransactionPool;
use std::{sync::Arc, time::Instant};
use tokio::sync::oneshot;
use tracing::{trace, warn};

/// The Engine API response sender.
pub type EngineApiSender<Ok> = oneshot::Sender<EngineApiResult<Ok>>;

/// The upper limit for payload bodies request.
const MAX_PAYLOAD_BODIES_LIMIT: u64 = 1024;

/// The upper limit blobs `eth_getBlobs`.
const MAX_BLOB_LIMIT: usize = 128;

/// The Engine API implementation that grants the Consensus layer access to data and
/// functions in the Execution layer that are crucial for the consensus process.
pub struct EngineApi<Provider, EngineT: EngineTypes, Pool, Validator, ChainSpec> {
    inner: Arc<EngineApiInner<Provider, EngineT, Pool, Validator, ChainSpec>>,
}

struct EngineApiInner<Provider, EngineT: EngineTypes, Pool, Validator, ChainSpec> {
    /// The provider to interact with the chain.
    provider: Provider,
    /// Consensus configuration
    chain_spec: Arc<ChainSpec>,
    /// The channel to send messages to the beacon consensus engine.
    beacon_consensus: BeaconConsensusEngineHandle<EngineT>,
    /// The type that can communicate with the payload service to retrieve payloads.
    payload_store: PayloadStore<EngineT>,
    /// For spawning and executing async tasks
    task_spawner: Box<dyn TaskSpawner>,
    /// The latency and response type metrics for engine api calls
    metrics: EngineApiMetrics,
    /// Identification of the execution client used by the consensus client
    client: ClientVersionV1,
    /// The list of all supported Engine capabilities available over the engine endpoint.
    capabilities: EngineCapabilities,
    /// Transaction pool.
    tx_pool: Pool,
    /// Engine validator.
    validator: Validator,
}

impl<Provider, EngineT, Pool, Validator, ChainSpec>
    EngineApi<Provider, EngineT, Pool, Validator, ChainSpec>
where
    Provider: HeaderProvider + BlockReader + StateProviderFactory + EvmEnvProvider + 'static,
    EngineT: EngineTypes,
    Pool: TransactionPool + 'static,
    Validator: EngineValidator<EngineT>,
    ChainSpec: EthereumHardforks + Send + Sync + 'static,
{
    /// Create new instance of [`EngineApi`].
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: Provider,
        chain_spec: Arc<ChainSpec>,
        beacon_consensus: BeaconConsensusEngineHandle<EngineT>,
        payload_store: PayloadStore<EngineT>,
        tx_pool: Pool,
        task_spawner: Box<dyn TaskSpawner>,
        client: ClientVersionV1,
        capabilities: EngineCapabilities,
        validator: Validator,
    ) -> Self {
        let inner = Arc::new(EngineApiInner {
            provider,
            chain_spec,
            beacon_consensus,
            payload_store,
            task_spawner,
            metrics: EngineApiMetrics::default(),
            client,
            capabilities,
            tx_pool,
            validator,
        });
        Self { inner }
    }

    /// Fetches the client version.
    fn get_client_version_v1(
        &self,
        _client: ClientVersionV1,
    ) -> EngineApiResult<Vec<ClientVersionV1>> {
        Ok(vec![self.inner.client.clone()])
    }
    /// Fetches the attributes for the payload with the given id.
    async fn get_payload_attributes(
        &self,
        payload_id: PayloadId,
    ) -> EngineApiResult<EngineT::PayloadBuilderAttributes> {
        Ok(self
            .inner
            .payload_store
            .payload_attributes(payload_id)
            .await
            .ok_or(EngineApiError::UnknownPayload)??)
    }

    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/paris.md#engine_newpayloadv1>
    /// Caution: This should not accept the `withdrawals` field
    pub async fn new_payload_v1(
        &self,
        payload: ExecutionPayloadV1,
    ) -> EngineApiResult<PayloadStatus> {
        let payload = ExecutionPayload::from(payload);
        let payload_or_attrs =
            PayloadOrAttributes::<'_, EngineT::PayloadAttributes>::from_execution_payload(
                &payload, None,
            );
        self.inner
            .validator
            .validate_version_specific_fields(EngineApiMessageVersion::V1, payload_or_attrs)?;
        Ok(self.inner.beacon_consensus.new_payload(payload, None).await?)
    }

    /// See also <https://github.com/ethereum/execution-apis/blob/584905270d8ad665718058060267061ecfd79ca5/src/engine/shanghai.md#engine_newpayloadv2>
    pub async fn new_payload_v2(
        &self,
        payload: ExecutionPayloadInputV2,
    ) -> EngineApiResult<PayloadStatus> {
        let payload = convert_payload_input_v2_to_payload(payload);
        let payload_or_attrs =
            PayloadOrAttributes::<'_, EngineT::PayloadAttributes>::from_execution_payload(
                &payload, None,
            );
        self.inner
            .validator
            .validate_version_specific_fields(EngineApiMessageVersion::V2, payload_or_attrs)?;
        Ok(self.inner.beacon_consensus.new_payload(payload, None).await?)
    }

    /// See also <https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md#engine_newpayloadv3>
    pub async fn new_payload_v3(
        &self,
        payload: ExecutionPayloadV3,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
    ) -> EngineApiResult<PayloadStatus> {
        let payload = ExecutionPayload::from(payload);
        let payload_or_attrs =
            PayloadOrAttributes::<'_, EngineT::PayloadAttributes>::from_execution_payload(
                &payload,
                Some(parent_beacon_block_root),
            );
        self.inner
            .validator
            .validate_version_specific_fields(EngineApiMessageVersion::V3, payload_or_attrs)?;

        let cancun_fields = CancunPayloadFields { versioned_hashes, parent_beacon_block_root };

        Ok(self.inner.beacon_consensus.new_payload(payload, Some(cancun_fields)).await?)
    }

    /// See also <https://github.com/ethereum/execution-apis/blob/7907424db935b93c2fe6a3c0faab943adebe8557/src/engine/prague.md#engine_newpayloadv4>
    pub async fn new_payload_v4(
        &self,
        payload: ExecutionPayloadV4,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
    ) -> EngineApiResult<PayloadStatus> {
        let payload = ExecutionPayload::from(payload);
        let payload_or_attrs =
            PayloadOrAttributes::<'_, EngineT::PayloadAttributes>::from_execution_payload(
                &payload,
                Some(parent_beacon_block_root),
            );
        self.inner
            .validator
            .validate_version_specific_fields(EngineApiMessageVersion::V4, payload_or_attrs)?;

        let cancun_fields = CancunPayloadFields { versioned_hashes, parent_beacon_block_root };

        Ok(self.inner.beacon_consensus.new_payload(payload, Some(cancun_fields)).await?)
    }

    /// Sends a message to the beacon consensus engine to update the fork choice _without_
    /// withdrawals.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/paris.md#engine_forkchoiceUpdatedV1>
    ///
    /// Caution: This should not accept the `withdrawals` field
    pub async fn fork_choice_updated_v1(
        &self,
        state: ForkchoiceState,
        payload_attrs: Option<EngineT::PayloadAttributes>,
    ) -> EngineApiResult<ForkchoiceUpdated> {
        self.validate_and_execute_forkchoice(EngineApiMessageVersion::V1, state, payload_attrs)
            .await
    }

    /// Sends a message to the beacon consensus engine to update the fork choice _with_ withdrawals,
    /// but only _after_ shanghai.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/shanghai.md#engine_forkchoiceupdatedv2>
    pub async fn fork_choice_updated_v2(
        &self,
        state: ForkchoiceState,
        payload_attrs: Option<EngineT::PayloadAttributes>,
    ) -> EngineApiResult<ForkchoiceUpdated> {
        self.validate_and_execute_forkchoice(EngineApiMessageVersion::V2, state, payload_attrs)
            .await
    }

    /// Sends a message to the beacon consensus engine to update the fork choice _with_ withdrawals,
    /// but only _after_ cancun.
    ///
    /// See also  <https://github.com/ethereum/execution-apis/blob/main/src/engine/cancun.md#engine_forkchoiceupdatedv3>
    pub async fn fork_choice_updated_v3(
        &self,
        state: ForkchoiceState,
        payload_attrs: Option<EngineT::PayloadAttributes>,
    ) -> EngineApiResult<ForkchoiceUpdated> {
        self.validate_and_execute_forkchoice(EngineApiMessageVersion::V3, state, payload_attrs)
            .await
    }

    /// Returns the most recent version of the payload that is available in the corresponding
    /// payload build process at the time of receiving this call.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/paris.md#engine_getPayloadV1>
    ///
    /// Caution: This should not return the `withdrawals` field
    ///
    /// Note:
    /// > Provider software MAY stop the corresponding build process after serving this call.
    pub async fn get_payload_v1(
        &self,
        payload_id: PayloadId,
    ) -> EngineApiResult<EngineT::ExecutionPayloadV1> {
        self.inner
            .payload_store
            .resolve(payload_id)
            .await
            .ok_or(EngineApiError::UnknownPayload)?
            .map_err(|_| EngineApiError::UnknownPayload)?
            .try_into()
            .map_err(|_| {
                warn!("could not transform built payload into ExecutionPayloadV1");
                EngineApiError::UnknownPayload
            })
    }

    /// Returns the most recent version of the payload that is available in the corresponding
    /// payload build process at the time of receiving this call.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/shanghai.md#engine_getpayloadv2>
    ///
    /// Note:
    /// > Provider software MAY stop the corresponding build process after serving this call.
    pub async fn get_payload_v2(
        &self,
        payload_id: PayloadId,
    ) -> EngineApiResult<EngineT::ExecutionPayloadV2> {
        // First we fetch the payload attributes to check the timestamp
        let attributes = self.get_payload_attributes(payload_id).await?;

        // validate timestamp according to engine rules
        validate_payload_timestamp(
            &self.inner.chain_spec,
            EngineApiMessageVersion::V2,
            attributes.timestamp(),
        )?;

        // Now resolve the payload
        self.inner
            .payload_store
            .resolve(payload_id)
            .await
            .ok_or(EngineApiError::UnknownPayload)?
            .map_err(|_| EngineApiError::UnknownPayload)?
            .try_into()
            .map_err(|_| {
                warn!("could not transform built payload into ExecutionPayloadV2");
                EngineApiError::UnknownPayload
            })
    }

    /// Returns the most recent version of the payload that is available in the corresponding
    /// payload build process at the time of receiving this call.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md#engine_getpayloadv3>
    ///
    /// Note:
    /// > Provider software MAY stop the corresponding build process after serving this call.
    pub async fn get_payload_v3(
        &self,
        payload_id: PayloadId,
    ) -> EngineApiResult<EngineT::ExecutionPayloadV3> {
        // First we fetch the payload attributes to check the timestamp
        let attributes = self.get_payload_attributes(payload_id).await?;

        // validate timestamp according to engine rules
        validate_payload_timestamp(
            &self.inner.chain_spec,
            EngineApiMessageVersion::V3,
            attributes.timestamp(),
        )?;

        // Now resolve the payload
        self.inner
            .payload_store
            .resolve(payload_id)
            .await
            .ok_or(EngineApiError::UnknownPayload)?
            .map_err(|_| EngineApiError::UnknownPayload)?
            .try_into()
            .map_err(|_| {
                warn!("could not transform built payload into ExecutionPayloadV3");
                EngineApiError::UnknownPayload
            })
    }

    /// Returns the most recent version of the payload that is available in the corresponding
    /// payload build process at the time of receiving this call.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/7907424db935b93c2fe6a3c0faab943adebe8557/src/engine/prague.md#engine_newpayloadv4>
    ///
    /// Note:
    /// > Provider software MAY stop the corresponding build process after serving this call.
    pub async fn get_payload_v4(
        &self,
        payload_id: PayloadId,
    ) -> EngineApiResult<EngineT::ExecutionPayloadV4> {
        // First we fetch the payload attributes to check the timestamp
        let attributes = self.get_payload_attributes(payload_id).await?;

        // validate timestamp according to engine rules
        validate_payload_timestamp(
            &self.inner.chain_spec,
            EngineApiMessageVersion::V4,
            attributes.timestamp(),
        )?;

        // Now resolve the payload
        self.inner
            .payload_store
            .resolve(payload_id)
            .await
            .ok_or(EngineApiError::UnknownPayload)?
            .map_err(|_| EngineApiError::UnknownPayload)?
            .try_into()
            .map_err(|_| {
                warn!("could not transform built payload into ExecutionPayloadV4");
                EngineApiError::UnknownPayload
            })
    }

    /// Fetches all the blocks for the provided range starting at `start`, containing `count`
    /// blocks and returns the mapped payload bodies.
    async fn get_payload_bodies_by_range_with<F, R>(
        &self,
        start: BlockNumber,
        count: u64,
        f: F,
    ) -> EngineApiResult<Vec<Option<R>>>
    where
        F: Fn(Block) -> R + Send + 'static,
        R: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let inner = self.inner.clone();

        self.inner.task_spawner.spawn_blocking(Box::pin(async move {
            if count > MAX_PAYLOAD_BODIES_LIMIT {
                tx.send(Err(EngineApiError::PayloadRequestTooLarge { len: count })).ok();
                return;
            }

            if start == 0 || count == 0 {
                tx.send(Err(EngineApiError::InvalidBodiesRange { start, count })).ok();
                return;
            }

            let mut result = Vec::with_capacity(count as usize);

            // -1 so range is inclusive
            let mut end = start.saturating_add(count - 1);

            // > Client software MUST NOT return trailing null values if the request extends past the current latest known block.
            // truncate the end if it's greater than the last block
            if let Ok(best_block) = inner.provider.best_block_number() {
                if end > best_block {
                    end = best_block;
                }
            }

            for num in start..=end {
                let block_result = inner.provider.block(BlockHashOrNumber::Number(num));
                match block_result {
                    Ok(block) => {
                        result.push(block.map(&f));
                    }
                    Err(err) => {
                        tx.send(Err(EngineApiError::Internal(Box::new(err)))).ok();
                        return;
                    }
                };
            }
            tx.send(Ok(result)).ok();
        }));

        rx.await.map_err(|err| EngineApiError::Internal(Box::new(err)))?
    }

    /// Returns the execution payload bodies by the range starting at `start`, containing `count`
    /// blocks.
    ///
    /// WARNING: This method is associated with the `BeaconBlocksByRange` message in the consensus
    /// layer p2p specification, meaning the input should be treated as untrusted or potentially
    /// adversarial.
    ///
    /// Implementers should take care when acting on the input to this method, specifically
    /// ensuring that the range is limited properly, and that the range boundaries are computed
    /// correctly and without panics.
    pub async fn get_payload_bodies_by_range_v1(
        &self,
        start: BlockNumber,
        count: u64,
    ) -> EngineApiResult<ExecutionPayloadBodiesV1> {
        self.get_payload_bodies_by_range_with(start, count, convert_to_payload_body_v1).await
    }

    /// Returns the execution payload bodies by the range starting at `start`, containing `count`
    /// blocks.
    ///
    /// Same as [`Self::get_payload_bodies_by_range_v1`] but as [`ExecutionPayloadBodiesV2`].
    pub async fn get_payload_bodies_by_range_v2(
        &self,
        start: BlockNumber,
        count: u64,
    ) -> EngineApiResult<ExecutionPayloadBodiesV2> {
        self.get_payload_bodies_by_range_with(start, count, convert_to_payload_body_v2).await
    }

    /// Called to retrieve execution payload bodies by hashes.
    async fn get_payload_bodies_by_hash_with<F, R>(
        &self,
        hashes: Vec<BlockHash>,
        f: F,
    ) -> EngineApiResult<Vec<Option<R>>>
    where
        F: Fn(Block) -> R + Send + 'static,
        R: Send + 'static,
    {
        let len = hashes.len() as u64;
        if len > MAX_PAYLOAD_BODIES_LIMIT {
            return Err(EngineApiError::PayloadRequestTooLarge { len });
        }

        let (tx, rx) = oneshot::channel();
        let inner = self.inner.clone();

        self.inner.task_spawner.spawn_blocking(Box::pin(async move {
            let mut result = Vec::with_capacity(hashes.len());
            for hash in hashes {
                let block_result = inner.provider.block(BlockHashOrNumber::Hash(hash));
                match block_result {
                    Ok(block) => {
                        result.push(block.map(&f));
                    }
                    Err(err) => {
                        let _ = tx.send(Err(EngineApiError::Internal(Box::new(err))));
                        return;
                    }
                }
            }
            tx.send(Ok(result)).ok();
        }));

        rx.await.map_err(|err| EngineApiError::Internal(Box::new(err)))?
    }

    /// Called to retrieve execution payload bodies by hashes.
    pub async fn get_payload_bodies_by_hash_v1(
        &self,
        hashes: Vec<BlockHash>,
    ) -> EngineApiResult<ExecutionPayloadBodiesV1> {
        self.get_payload_bodies_by_hash_with(hashes, convert_to_payload_body_v1).await
    }

    /// Called to retrieve execution payload bodies by hashes.
    ///
    /// Same as [`Self::get_payload_bodies_by_hash_v1`] but as [`ExecutionPayloadBodiesV2`].
    pub async fn get_payload_bodies_by_hash_v2(
        &self,
        hashes: Vec<BlockHash>,
    ) -> EngineApiResult<ExecutionPayloadBodiesV2> {
        self.get_payload_bodies_by_hash_with(hashes, convert_to_payload_body_v2).await
    }

    /// Called to verify network configuration parameters and ensure that Consensus and Execution
    /// layers are using the latest configuration.
    pub fn exchange_transition_configuration(
        &self,
        config: TransitionConfiguration,
    ) -> EngineApiResult<TransitionConfiguration> {
        let TransitionConfiguration {
            terminal_total_difficulty,
            terminal_block_hash,
            terminal_block_number,
        } = config;

        let merge_terminal_td = self
            .inner
            .chain_spec
            .fork(EthereumHardfork::Paris)
            .ttd()
            .expect("the engine API should not be running for chains w/o paris");

        // Compare total difficulty values
        if merge_terminal_td != terminal_total_difficulty {
            return Err(EngineApiError::TerminalTD {
                execution: merge_terminal_td,
                consensus: terminal_total_difficulty,
            })
        }

        self.inner.beacon_consensus.transition_configuration_exchanged();

        // Short circuit if communicated block hash is zero
        if terminal_block_hash.is_zero() {
            return Ok(TransitionConfiguration {
                terminal_total_difficulty: merge_terminal_td,
                ..Default::default()
            })
        }

        // Attempt to look up terminal block hash
        let local_hash = self
            .inner
            .provider
            .block_hash(terminal_block_number)
            .map_err(|err| EngineApiError::Internal(Box::new(err)))?;

        // Transition configuration exchange is successful if block hashes match
        match local_hash {
            Some(hash) if hash == terminal_block_hash => Ok(TransitionConfiguration {
                terminal_total_difficulty: merge_terminal_td,
                terminal_block_hash,
                terminal_block_number,
            }),
            _ => Err(EngineApiError::TerminalBlockHash {
                execution: local_hash,
                consensus: terminal_block_hash,
            }),
        }
    }

    /// Validates the `engine_forkchoiceUpdated` payload attributes and executes the forkchoice
    /// update.
    ///
    /// The payload attributes will be validated according to the engine API rules for the given
    /// message version:
    /// * If the version is [`EngineApiMessageVersion::V1`], then the payload attributes will be
    ///   validated according to the Paris rules.
    /// * If the version is [`EngineApiMessageVersion::V2`], then the payload attributes will be
    ///   validated according to the Shanghai rules, as well as the validity changes from cancun:
    ///   <https://github.com/ethereum/execution-apis/blob/584905270d8ad665718058060267061ecfd79ca5/src/engine/cancun.md#update-the-methods-of-previous-forks>
    ///
    /// * If the version above [`EngineApiMessageVersion::V3`], then the payload attributes will be
    ///   validated according to the Cancun rules.
    async fn validate_and_execute_forkchoice(
        &self,
        version: EngineApiMessageVersion,
        state: ForkchoiceState,
        payload_attrs: Option<EngineT::PayloadAttributes>,
    ) -> EngineApiResult<ForkchoiceUpdated> {
        if let Some(ref attrs) = payload_attrs {
            let attr_validation_res =
                self.inner.validator.ensure_well_formed_attributes(version, attrs);

            // From the engine API spec:
            //
            // Client software MUST ensure that payloadAttributes.timestamp is greater than
            // timestamp of a block referenced by forkchoiceState.headBlockHash. If this condition
            // isn't held client software MUST respond with -38003: Invalid payload attributes and
            // MUST NOT begin a payload build process. In such an event, the forkchoiceState
            // update MUST NOT be rolled back.
            //
            // NOTE: This will also apply to the validation result for the cancun or
            // shanghai-specific fields provided in the payload attributes.
            //
            // To do this, we set the payload attrs to `None` if attribute validation failed, but
            // we still apply the forkchoice update.
            if let Err(err) = attr_validation_res {
                let fcu_res = self.inner.beacon_consensus.fork_choice_updated(state, None).await?;
                // TODO: decide if we want this branch - the FCU INVALID response might be more
                // useful than the payload attributes INVALID response
                if fcu_res.is_invalid() {
                    return Ok(fcu_res)
                }
                return Err(err.into())
            }
        }

        Ok(self.inner.beacon_consensus.fork_choice_updated(state, payload_attrs).await?)
    }
}

#[async_trait]
impl<Provider, EngineT, Pool, Validator, ChainSpec> EngineApiServer<EngineT>
    for EngineApi<Provider, EngineT, Pool, Validator, ChainSpec>
where
    Provider: HeaderProvider + BlockReader + StateProviderFactory + EvmEnvProvider + 'static,
    EngineT: EngineTypes,
    Pool: TransactionPool + 'static,
    Validator: EngineValidator<EngineT>,
    ChainSpec: EthereumHardforks + Send + Sync + 'static,
{
    /// Handler for `engine_newPayloadV1`
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/paris.md#engine_newpayloadv1>
    /// Caution: This should not accept the `withdrawals` field
    async fn new_payload_v1(&self, payload: ExecutionPayloadV1) -> RpcResult<PayloadStatus> {
        trace!(target: "rpc::engine", "Serving engine_newPayloadV1");
        let start = Instant::now();
        let gas_used = payload.gas_used;
        let res = Self::new_payload_v1(self, payload).await;
        let elapsed = start.elapsed();
        self.inner.metrics.latency.new_payload_v1.record(elapsed);
        self.inner.metrics.new_payload_response.update_response_metrics(&res, gas_used, elapsed);
        Ok(res?)
    }

    /// Handler for `engine_newPayloadV2`
    /// See also <https://github.com/ethereum/execution-apis/blob/584905270d8ad665718058060267061ecfd79ca5/src/engine/shanghai.md#engine_newpayloadv2>
    async fn new_payload_v2(&self, payload: ExecutionPayloadInputV2) -> RpcResult<PayloadStatus> {
        trace!(target: "rpc::engine", "Serving engine_newPayloadV2");
        let start = Instant::now();
        let gas_used = payload.execution_payload.gas_used;
        let res = Self::new_payload_v2(self, payload).await;
        let elapsed = start.elapsed();
        self.inner.metrics.latency.new_payload_v2.record(elapsed);
        self.inner.metrics.new_payload_response.update_response_metrics(&res, gas_used, elapsed);
        Ok(res?)
    }

    /// Handler for `engine_newPayloadV3`
    /// See also <https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md#engine_newpayloadv3>
    async fn new_payload_v3(
        &self,
        payload: ExecutionPayloadV3,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
    ) -> RpcResult<PayloadStatus> {
        trace!(target: "rpc::engine", "Serving engine_newPayloadV3");
        let start = Instant::now();
        let gas_used = payload.payload_inner.payload_inner.gas_used;
        let res =
            Self::new_payload_v3(self, payload, versioned_hashes, parent_beacon_block_root).await;
        let elapsed = start.elapsed();
        self.inner.metrics.latency.new_payload_v3.record(elapsed);
        self.inner.metrics.new_payload_response.update_response_metrics(&res, gas_used, elapsed);
        Ok(res?)
    }

    /// Handler for `engine_newPayloadV4`
    /// See also <https://github.com/ethereum/execution-apis/blob/03911ffc053b8b806123f1fc237184b0092a485a/src/engine/prague.md#engine_newpayloadv4>
    async fn new_payload_v4(
        &self,
        payload: ExecutionPayloadV4,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
    ) -> RpcResult<PayloadStatus> {
        trace!(target: "rpc::engine", "Serving engine_newPayloadV4");
        let start = Instant::now();
        let gas_used = payload.payload_inner.payload_inner.payload_inner.gas_used;
        let res =
            Self::new_payload_v4(self, payload, versioned_hashes, parent_beacon_block_root).await;
        let elapsed = start.elapsed();
        self.inner.metrics.latency.new_payload_v4.record(elapsed);
        self.inner.metrics.new_payload_response.update_response_metrics(&res, gas_used, elapsed);
        Ok(res?)
    }

    /// Handler for `engine_forkchoiceUpdatedV1`
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/paris.md#engine_forkchoiceupdatedv1>
    ///
    /// Caution: This should not accept the `withdrawals` field
    async fn fork_choice_updated_v1(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<EngineT::PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        trace!(target: "rpc::engine", "Serving engine_forkchoiceUpdatedV1");
        let start = Instant::now();
        let res = Self::fork_choice_updated_v1(self, fork_choice_state, payload_attributes).await;
        self.inner.metrics.latency.fork_choice_updated_v1.record(start.elapsed());
        self.inner.metrics.fcu_response.update_response_metrics(&res);
        Ok(res?)
    }

    /// Handler for `engine_forkchoiceUpdatedV2`
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/shanghai.md#engine_forkchoiceupdatedv2>
    async fn fork_choice_updated_v2(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<EngineT::PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        trace!(target: "rpc::engine", "Serving engine_forkchoiceUpdatedV2");
        let start = Instant::now();
        let res = Self::fork_choice_updated_v2(self, fork_choice_state, payload_attributes).await;
        self.inner.metrics.latency.fork_choice_updated_v2.record(start.elapsed());
        self.inner.metrics.fcu_response.update_response_metrics(&res);
        Ok(res?)
    }

    /// Handler for `engine_forkchoiceUpdatedV2`
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/cancun.md#engine_forkchoiceupdatedv3>
    async fn fork_choice_updated_v3(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<EngineT::PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        trace!(target: "rpc::engine", "Serving engine_forkchoiceUpdatedV3");
        let start = Instant::now();
        let res = Self::fork_choice_updated_v3(self, fork_choice_state, payload_attributes).await;
        self.inner.metrics.latency.fork_choice_updated_v3.record(start.elapsed());
        self.inner.metrics.fcu_response.update_response_metrics(&res);
        Ok(res?)
    }

    /// Handler for `engine_getPayloadV1`
    ///
    /// Returns the most recent version of the payload that is available in the corresponding
    /// payload build process at the time of receiving this call.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/paris.md#engine_getPayloadV1>
    ///
    /// Caution: This should not return the `withdrawals` field
    ///
    /// Note:
    /// > Provider software MAY stop the corresponding build process after serving this call.
    async fn get_payload_v1(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<EngineT::ExecutionPayloadV1> {
        trace!(target: "rpc::engine", "Serving engine_getPayloadV1");
        let start = Instant::now();
        let res = Self::get_payload_v1(self, payload_id).await;
        self.inner.metrics.latency.get_payload_v1.record(start.elapsed());
        Ok(res?)
    }

    /// Handler for `engine_getPayloadV2`
    ///
    /// Returns the most recent version of the payload that is available in the corresponding
    /// payload build process at the time of receiving this call.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/shanghai.md#engine_getpayloadv2>
    ///
    /// Note:
    /// > Provider software MAY stop the corresponding build process after serving this call.
    async fn get_payload_v2(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<EngineT::ExecutionPayloadV2> {
        trace!(target: "rpc::engine", "Serving engine_getPayloadV2");
        let start = Instant::now();
        let res = Self::get_payload_v2(self, payload_id).await;
        self.inner.metrics.latency.get_payload_v2.record(start.elapsed());
        Ok(res?)
    }

    /// Handler for `engine_getPayloadV3`
    ///
    /// Returns the most recent version of the payload that is available in the corresponding
    /// payload build process at the time of receiving this call.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md#engine_getpayloadv3>
    ///
    /// Note:
    /// > Provider software MAY stop the corresponding build process after serving this call.
    async fn get_payload_v3(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<EngineT::ExecutionPayloadV3> {
        trace!(target: "rpc::engine", "Serving engine_getPayloadV3");
        let start = Instant::now();
        let res = Self::get_payload_v3(self, payload_id).await;
        self.inner.metrics.latency.get_payload_v3.record(start.elapsed());
        Ok(res?)
    }

    /// Handler for `engine_getPayloadV4`
    ///
    /// Returns the most recent version of the payload that is available in the corresponding
    /// payload build process at the time of receiving this call.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/prague.md#engine_getpayloadv4>
    ///
    /// Note:
    /// > Provider software MAY stop the corresponding build process after serving this call.
    async fn get_payload_v4(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<EngineT::ExecutionPayloadV4> {
        trace!(target: "rpc::engine", "Serving engine_getPayloadV4");
        let start = Instant::now();
        let res = Self::get_payload_v4(self, payload_id).await;
        self.inner.metrics.latency.get_payload_v4.record(start.elapsed());
        Ok(res?)
    }

    /// Handler for `engine_getPayloadBodiesByHashV1`
    /// See also <https://github.com/ethereum/execution-apis/blob/6452a6b194d7db269bf1dbd087a267251d3cc7f8/src/engine/shanghai.md#engine_getpayloadbodiesbyhashv1>
    async fn get_payload_bodies_by_hash_v1(
        &self,
        block_hashes: Vec<BlockHash>,
    ) -> RpcResult<ExecutionPayloadBodiesV1> {
        trace!(target: "rpc::engine", "Serving engine_getPayloadBodiesByHashV1");
        let start = Instant::now();
        let res = Self::get_payload_bodies_by_hash_v1(self, block_hashes);
        self.inner.metrics.latency.get_payload_bodies_by_hash_v1.record(start.elapsed());
        Ok(res.await?)
    }

    async fn get_payload_bodies_by_hash_v2(
        &self,
        block_hashes: Vec<BlockHash>,
    ) -> RpcResult<ExecutionPayloadBodiesV2> {
        trace!(target: "rpc::engine", "Serving engine_getPayloadBodiesByHashV2");
        let start = Instant::now();
        let res = Self::get_payload_bodies_by_hash_v2(self, block_hashes);
        self.inner.metrics.latency.get_payload_bodies_by_hash_v2.record(start.elapsed());
        Ok(res.await?)
    }

    /// Handler for `engine_getPayloadBodiesByRangeV1`
    ///
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
    ///
    /// Note: If a block is pre shanghai, `withdrawals` field will be `null`.
    async fn get_payload_bodies_by_range_v1(
        &self,
        start: U64,
        count: U64,
    ) -> RpcResult<ExecutionPayloadBodiesV1> {
        trace!(target: "rpc::engine", "Serving engine_getPayloadBodiesByRangeV1");
        let start_time = Instant::now();
        let res = Self::get_payload_bodies_by_range_v1(self, start.to(), count.to()).await;
        self.inner.metrics.latency.get_payload_bodies_by_range_v1.record(start_time.elapsed());
        Ok(res?)
    }

    async fn get_payload_bodies_by_range_v2(
        &self,
        start: U64,
        count: U64,
    ) -> RpcResult<ExecutionPayloadBodiesV2> {
        trace!(target: "rpc::engine", "Serving engine_getPayloadBodiesByRangeV2");
        let start_time = Instant::now();
        let res = Self::get_payload_bodies_by_range_v2(self, start.to(), count.to()).await;
        self.inner.metrics.latency.get_payload_bodies_by_range_v2.record(start_time.elapsed());
        Ok(res?)
    }

    /// Handler for `engine_exchangeTransitionConfigurationV1`
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/paris.md#engine_exchangeTransitionConfigurationV1>
    async fn exchange_transition_configuration(
        &self,
        config: TransitionConfiguration,
    ) -> RpcResult<TransitionConfiguration> {
        trace!(target: "rpc::engine", "Serving engine_exchangeTransitionConfigurationV1");
        let start = Instant::now();
        let res = Self::exchange_transition_configuration(self, config);
        self.inner.metrics.latency.exchange_transition_configuration.record(start.elapsed());
        Ok(res?)
    }

    /// Handler for `engine_getClientVersionV1`
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/03911ffc053b8b806123f1fc237184b0092a485a/src/engine/identification.md>
    async fn get_client_version_v1(
        &self,
        client: ClientVersionV1,
    ) -> RpcResult<Vec<ClientVersionV1>> {
        trace!(target: "rpc::engine", "Serving engine_getClientVersionV1");
        let res = Self::get_client_version_v1(self, client);

        Ok(res?)
    }

    /// Handler for `engine_exchangeCapabilitiesV1`
    /// See also <https://github.com/ethereum/execution-apis/blob/6452a6b194d7db269bf1dbd087a267251d3cc7f8/src/engine/common.md#capabilities>
    async fn exchange_capabilities(&self, _capabilities: Vec<String>) -> RpcResult<Vec<String>> {
        Ok(self.inner.capabilities.list())
    }

    async fn get_blobs_v1(
        &self,
        versioned_hashes: Vec<B256>,
    ) -> RpcResult<Vec<Option<BlobAndProofV1>>> {
        trace!(target: "rpc::engine", "Serving engine_getBlobsV1");
        if versioned_hashes.len() > MAX_BLOB_LIMIT {
            return Err(EngineApiError::BlobRequestTooLarge { len: versioned_hashes.len() }.into())
        }

        Ok(self
            .inner
            .tx_pool
            .get_blobs_for_versioned_hashes(&versioned_hashes)
            .map_err(|err| EngineApiError::Internal(Box::new(err)))?)
    }
}

impl<Provider, EngineT, Pool, Validator, ChainSpec> std::fmt::Debug
    for EngineApi<Provider, EngineT, Pool, Validator, ChainSpec>
where
    EngineT: EngineTypes,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineApi").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_rpc_types_engine::{ClientCode, ClientVersionV1};
    use assert_matches::assert_matches;
    use reth_beacon_consensus::{BeaconConsensusEngineEvent, BeaconEngineMessage};
    use reth_chainspec::{ChainSpec, MAINNET};
    use reth_ethereum_engine_primitives::{EthEngineTypes, EthereumEngineValidator};
    use reth_payload_builder::test_utils::spawn_test_payload_service;
    use reth_primitives::SealedBlock;
    use reth_provider::test_utils::MockEthProvider;
    use reth_rpc_types_compat::engine::payload::execution_payload_from_sealed_block;
    use reth_tasks::TokioTaskExecutor;
    use reth_testing_utils::generators::random_block;
    use reth_tokio_util::EventSender;
    use reth_transaction_pool::noop::NoopTransactionPool;
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

    fn setup_engine_api() -> (
        EngineApiTestHandle,
        EngineApi<
            Arc<MockEthProvider>,
            EthEngineTypes,
            NoopTransactionPool,
            EthereumEngineValidator,
            ChainSpec,
        >,
    ) {
        let client = ClientVersionV1 {
            code: ClientCode::RH,
            name: "Reth".to_string(),
            version: "v0.2.0-beta.5".to_string(),
            commit: "defa64b2".to_string(),
        };

        let chain_spec: Arc<ChainSpec> = MAINNET.clone();
        let provider = Arc::new(MockEthProvider::default());
        let payload_store = spawn_test_payload_service();
        let (to_engine, engine_rx) = unbounded_channel();
        let event_sender: EventSender<BeaconConsensusEngineEvent> = Default::default();
        let task_executor = Box::<TokioTaskExecutor>::default();
        let api = EngineApi::new(
            provider.clone(),
            chain_spec.clone(),
            BeaconConsensusEngineHandle::new(to_engine, event_sender),
            payload_store.into(),
            NoopTransactionPool::default(),
            task_executor,
            client,
            EngineCapabilities::default(),
            EthereumEngineValidator::new(chain_spec.clone()),
        );
        let handle = EngineApiTestHandle { chain_spec, provider, from_api: engine_rx };
        (handle, api)
    }

    #[tokio::test]
    async fn engine_client_version_v1() {
        let client = ClientVersionV1 {
            code: ClientCode::RH,
            name: "Reth".to_string(),
            version: "v0.2.0-beta.5".to_string(),
            commit: "defa64b2".to_string(),
        };
        let (_, api) = setup_engine_api();
        let res = api.get_client_version_v1(client.clone());
        assert_eq!(res.unwrap(), vec![client]);
    }

    struct EngineApiTestHandle {
        chain_spec: Arc<ChainSpec>,
        provider: Arc<MockEthProvider>,
        from_api: UnboundedReceiver<BeaconEngineMessage<EthEngineTypes>>,
    }

    #[tokio::test]
    async fn forwards_responses_to_consensus_engine() {
        let (mut handle, api) = setup_engine_api();

        tokio::spawn(async move {
            api.new_payload_v1(execution_payload_from_sealed_block(SealedBlock::default()))
                .await
                .unwrap();
        });
        assert_matches!(handle.from_api.recv().await, Some(BeaconEngineMessage::NewPayload { .. }));
    }

    // tests covering `engine_getPayloadBodiesByRange` and `engine_getPayloadBodiesByHash`
    mod get_payload_bodies {
        use super::*;
        use reth_testing_utils::generators::{self, random_block_range, BlockRangeParams};

        #[tokio::test]
        async fn invalid_params() {
            let (_, api) = setup_engine_api();

            let by_range_tests = [
                // (start, count)
                (0, 0),
                (0, 1),
                (1, 0),
            ];

            // test [EngineApiMessage::GetPayloadBodiesByRange]
            for (start, count) in by_range_tests {
                let res = api.get_payload_bodies_by_range_v1(start, count).await;
                assert_matches!(res, Err(EngineApiError::InvalidBodiesRange { .. }));
            }
        }

        #[tokio::test]
        async fn request_too_large() {
            let (_, api) = setup_engine_api();

            let request_count = MAX_PAYLOAD_BODIES_LIMIT + 1;
            let res = api.get_payload_bodies_by_range_v1(0, request_count).await;
            assert_matches!(res, Err(EngineApiError::PayloadRequestTooLarge { .. }));
        }

        #[tokio::test]
        async fn returns_payload_bodies() {
            let mut rng = generators::rng();
            let (handle, api) = setup_engine_api();

            let (start, count) = (1, 10);
            let blocks = random_block_range(
                &mut rng,
                start..=start + count - 1,
                BlockRangeParams { tx_count: 0..2, ..Default::default() },
            );
            handle.provider.extend_blocks(blocks.iter().cloned().map(|b| (b.hash(), b.unseal())));

            let expected = blocks
                .iter()
                .cloned()
                .map(|b| Some(convert_to_payload_body_v1(b.unseal())))
                .collect::<Vec<_>>();

            let res = api.get_payload_bodies_by_range_v1(start, count).await.unwrap();
            assert_eq!(res, expected);
        }

        #[tokio::test]
        async fn returns_payload_bodies_with_gaps() {
            let mut rng = generators::rng();
            let (handle, api) = setup_engine_api();

            let (start, count) = (1, 100);
            let blocks = random_block_range(
                &mut rng,
                start..=start + count - 1,
                BlockRangeParams { tx_count: 0..2, ..Default::default() },
            );

            // Insert only blocks in ranges 1-25 and 50-75
            let first_missing_range = 26..=50;
            let second_missing_range = 76..=100;
            handle.provider.extend_blocks(
                blocks
                    .iter()
                    .filter(|b| {
                        !first_missing_range.contains(&b.number) &&
                            !second_missing_range.contains(&b.number)
                    })
                    .map(|b| (b.hash(), b.clone().unseal())),
            );

            let expected = blocks
                .iter()
                // filter anything after the second missing range to ensure we don't expect trailing
                // `None`s
                .filter(|b| !second_missing_range.contains(&b.number))
                .cloned()
                .map(|b| {
                    if first_missing_range.contains(&b.number) {
                        None
                    } else {
                        Some(convert_to_payload_body_v1(b.unseal()))
                    }
                })
                .collect::<Vec<_>>();

            let res = api.get_payload_bodies_by_range_v1(start, count).await.unwrap();
            assert_eq!(res, expected);

            let expected = blocks
                .iter()
                .cloned()
                // ensure we still return trailing `None`s here because by-hash will not be aware
                // of the missing block's number, and cannot compare it to the current best block
                .map(|b| {
                    if first_missing_range.contains(&b.number) ||
                        second_missing_range.contains(&b.number)
                    {
                        None
                    } else {
                        Some(convert_to_payload_body_v1(b.unseal()))
                    }
                })
                .collect::<Vec<_>>();

            let hashes = blocks.iter().map(|b| b.hash()).collect();
            let res = api.get_payload_bodies_by_hash_v1(hashes).await.unwrap();
            assert_eq!(res, expected);
        }
    }

    // https://github.com/ethereum/execution-apis/blob/main/src/engine/paris.md#specification-3
    mod exchange_transition_configuration {
        use super::*;
        use alloy_primitives::U256;
        use reth_testing_utils::generators::{self, BlockParams};

        #[tokio::test]
        async fn terminal_td_mismatch() {
            let (handle, api) = setup_engine_api();

            let transition_config = TransitionConfiguration {
                terminal_total_difficulty: handle
                    .chain_spec
                    .fork(EthereumHardfork::Paris)
                    .ttd()
                    .unwrap() +
                    U256::from(1),
                ..Default::default()
            };

            let res = api.exchange_transition_configuration(transition_config);

            assert_matches!(
                res,
                Err(EngineApiError::TerminalTD { execution, consensus })
                    if execution == handle.chain_spec.fork(EthereumHardfork::Paris).ttd().unwrap() && consensus == U256::from(transition_config.terminal_total_difficulty)
            );
        }

        #[tokio::test]
        async fn terminal_block_hash_mismatch() {
            let mut rng = generators::rng();

            let (handle, api) = setup_engine_api();

            let terminal_block_number = 1000;
            let consensus_terminal_block =
                random_block(&mut rng, terminal_block_number, BlockParams::default());
            let execution_terminal_block =
                random_block(&mut rng, terminal_block_number, BlockParams::default());

            let transition_config = TransitionConfiguration {
                terminal_total_difficulty: handle
                    .chain_spec
                    .fork(EthereumHardfork::Paris)
                    .ttd()
                    .unwrap(),
                terminal_block_hash: consensus_terminal_block.hash(),
                terminal_block_number,
            };

            // Unknown block number
            let res = api.exchange_transition_configuration(transition_config);

            assert_matches!(
               res,
                Err(EngineApiError::TerminalBlockHash { execution, consensus })
                    if execution.is_none() && consensus == transition_config.terminal_block_hash
            );

            // Add block and to provider local store and test for mismatch
            handle.provider.add_block(
                execution_terminal_block.hash(),
                execution_terminal_block.clone().unseal(),
            );

            let res = api.exchange_transition_configuration(transition_config);

            assert_matches!(
                res,
                Err(EngineApiError::TerminalBlockHash { execution, consensus })
                    if execution == Some(execution_terminal_block.hash()) && consensus == transition_config.terminal_block_hash
            );
        }

        #[tokio::test]
        async fn configurations_match() {
            let (handle, api) = setup_engine_api();

            let terminal_block_number = 1000;
            let terminal_block =
                random_block(&mut generators::rng(), terminal_block_number, BlockParams::default());

            let transition_config = TransitionConfiguration {
                terminal_total_difficulty: handle
                    .chain_spec
                    .fork(EthereumHardfork::Paris)
                    .ttd()
                    .unwrap(),
                terminal_block_hash: terminal_block.hash(),
                terminal_block_number,
            };

            handle.provider.add_block(terminal_block.hash(), terminal_block.unseal());

            let config = api.exchange_transition_configuration(transition_config).unwrap();
            assert_eq!(config, transition_config);
        }
    }
}

use crate::{
    metrics::EngineApiMetrics, payload::PayloadOrAttributes, EngineApiError,
    EngineApiMessageVersion, EngineApiResult,
};
use async_trait::async_trait;
use jsonrpsee_core::RpcResult;
use reth_beacon_consensus::BeaconConsensusEngineHandle;
use reth_interfaces::consensus::ForkchoiceState;
use reth_payload_builder::{
    EngineTypes, PayloadAttributesTrait, PayloadBuilderAttributesTrait, PayloadStore,
};
use reth_primitives::{BlockHash, BlockHashOrNumber, BlockNumber, ChainSpec, Hardfork, B256, U64};
use reth_provider::{BlockReader, EvmEnvProvider, HeaderProvider, StateProviderFactory};
use reth_rpc_api::EngineApiServer;
use reth_rpc_types::engine::{
    CancunPayloadFields, ExecutionPayload, ExecutionPayloadBodiesV1, ExecutionPayloadEnvelopeV2,
    ExecutionPayloadEnvelopeV3, ExecutionPayloadInputV2, ExecutionPayloadV1, ExecutionPayloadV3,
    ForkchoiceUpdated, PayloadId, PayloadStatus, TransitionConfiguration, CAPABILITIES,
};
use reth_rpc_types_compat::engine::payload::{
    convert_payload_input_v2_to_payload, convert_to_payload_body_v1,
};
use reth_tasks::TaskSpawner;
use std::{sync::Arc, time::Instant};
use tokio::sync::oneshot;
use tracing::trace;

/// The Engine API response sender.
pub type EngineApiSender<Ok> = oneshot::Sender<EngineApiResult<Ok>>;

/// The upper limit for payload bodies request.
const MAX_PAYLOAD_BODIES_LIMIT: u64 = 1024;

/// The Engine API implementation that grants the Consensus layer access to data and
/// functions in the Execution layer that are crucial for the consensus process.
pub struct EngineApi<Provider, Types: EngineTypes> {
    inner: Arc<EngineApiInner<Provider, Types>>,
}

struct EngineApiInner<Provider, Types: EngineTypes> {
    /// The provider to interact with the chain.
    provider: Provider,
    /// Consensus configuration
    chain_spec: Arc<ChainSpec>,
    /// The channel to send messages to the beacon consensus engine.
    beacon_consensus: BeaconConsensusEngineHandle<Types>,
    /// The type that can communicate with the payload service to retrieve payloads.
    payload_store: PayloadStore<Types>,
    /// For spawning and executing async tasks
    task_spawner: Box<dyn TaskSpawner>,
    /// The metrics for engine api calls
    metrics: EngineApiMetrics,
}

impl<Provider, Types> EngineApi<Provider, Types>
where
    Provider: HeaderProvider + BlockReader + StateProviderFactory + EvmEnvProvider + 'static,
    Types: EngineTypes + 'static,
    Types::PayloadBuilderAttributes: Send,
{
    /// Create new instance of [EngineApi].
    pub fn new(
        provider: Provider,
        chain_spec: Arc<ChainSpec>,
        beacon_consensus: BeaconConsensusEngineHandle<Types>,
        payload_store: PayloadStore<Types>,
        task_spawner: Box<dyn TaskSpawner>,
    ) -> Self {
        let inner = Arc::new(EngineApiInner {
            provider,
            chain_spec,
            beacon_consensus,
            payload_store,
            task_spawner,
            metrics: EngineApiMetrics::default(),
        });
        Self { inner }
    }

    /// Fetches the attributes for the payload with the given id.
    async fn get_payload_attributes(
        &self,
        payload_id: PayloadId,
    ) -> EngineApiResult<Types::PayloadBuilderAttributes> {
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
            PayloadOrAttributes::<'_, Types::PayloadAttributes>::from_execution_payload(
                &payload, None,
            );
        self.validate_version_specific_fields(EngineApiMessageVersion::V1, &payload_or_attrs)?;
        Ok(self.inner.beacon_consensus.new_payload(payload, None).await?)
    }

    /// See also <https://github.com/ethereum/execution-apis/blob/584905270d8ad665718058060267061ecfd79ca5/src/engine/shanghai.md#engine_newpayloadv2>
    pub async fn new_payload_v2(
        &self,
        payload: ExecutionPayloadInputV2,
    ) -> EngineApiResult<PayloadStatus> {
        let payload = convert_payload_input_v2_to_payload(payload);
        let payload_or_attrs =
            PayloadOrAttributes::<'_, Types::PayloadAttributes>::from_execution_payload(
                &payload, None,
            );
        self.validate_version_specific_fields(EngineApiMessageVersion::V2, &payload_or_attrs)?;
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
            PayloadOrAttributes::<'_, Types::PayloadAttributes>::from_execution_payload(
                &payload,
                Some(parent_beacon_block_root),
            );
        self.validate_version_specific_fields(EngineApiMessageVersion::V3, &payload_or_attrs)?;

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
        payload_attrs: Option<Types::PayloadAttributes>,
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
        payload_attrs: Option<Types::PayloadAttributes>,
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
        payload_attrs: Option<Types::PayloadAttributes>,
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
    ) -> EngineApiResult<ExecutionPayloadV1> {
        Ok(self
            .inner
            .payload_store
            .resolve(payload_id)
            .await
            .ok_or(EngineApiError::UnknownPayload)?
            .map(|payload| (*payload).clone().into_v1_payload())?)
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
    ) -> EngineApiResult<ExecutionPayloadEnvelopeV2> {
        // First we fetch the payload attributes to check the timestamp
        let attributes = self.get_payload_attributes(payload_id).await?;

        // validate timestamp according to engine rules
        self.validate_payload_timestamp(EngineApiMessageVersion::V2, attributes.timestamp())?;

        // Now resolve the payload
        Ok(self
            .inner
            .payload_store
            .resolve(payload_id)
            .await
            .ok_or(EngineApiError::UnknownPayload)?
            .map(|payload| (*payload).clone().into_v2_payload())?)
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
    ) -> EngineApiResult<ExecutionPayloadEnvelopeV3> {
        // First we fetch the payload attributes to check the timestamp
        let attributes = self.get_payload_attributes(payload_id).await?;

        // validate timestamp according to engine rules
        self.validate_payload_timestamp(EngineApiMessageVersion::V3, attributes.timestamp())?;

        // Now resolve the payload
        Ok(self
            .inner
            .payload_store
            .resolve(payload_id)
            .await
            .ok_or(EngineApiError::UnknownPayload)?
            .map(|payload| (*payload).clone().into_v3_payload())?)
    }

    /// Returns the execution payload bodies by the range starting at `start`, containing `count`
    /// blocks.
    ///
    /// WARNING: This method is associated with the BeaconBlocksByRange message in the consensus
    /// layer p2p specification, meaning the input should be treated as untrusted or potentially
    /// adversarial.
    ///
    /// Implementors should take care when acting on the input to this method, specifically
    /// ensuring that the range is limited properly, and that the range boundaries are computed
    /// correctly and without panics.
    pub async fn get_payload_bodies_by_range(
        &self,
        start: BlockNumber,
        count: u64,
    ) -> EngineApiResult<ExecutionPayloadBodiesV1> {
        let (tx, rx) = oneshot::channel();
        let inner = self.inner.clone();

        self.inner.task_spawner.spawn_blocking(Box::pin(async move {
            if count > MAX_PAYLOAD_BODIES_LIMIT {
                tx.send(Err(EngineApiError::PayloadRequestTooLarge { len: count })).ok();
                return
            }

            if start == 0 || count == 0 {
                tx.send(Err(EngineApiError::InvalidBodiesRange { start, count })).ok();
                return
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
                        result.push(block.map(convert_to_payload_body_v1));
                    }
                    Err(err) => {
                        tx.send(Err(EngineApiError::Internal(Box::new(err)))).ok();
                        return
                    }
                };
            }
            tx.send(Ok(result)).ok();
        }));

        rx.await.map_err(|err| EngineApiError::Internal(Box::new(err)))?
    }

    /// Called to retrieve execution payload bodies by hashes.
    pub fn get_payload_bodies_by_hash(
        &self,
        hashes: Vec<BlockHash>,
    ) -> EngineApiResult<ExecutionPayloadBodiesV1> {
        let len = hashes.len() as u64;
        if len > MAX_PAYLOAD_BODIES_LIMIT {
            return Err(EngineApiError::PayloadRequestTooLarge { len })
        }

        let mut result = Vec::with_capacity(hashes.len());
        for hash in hashes {
            let block = self
                .inner
                .provider
                .block(BlockHashOrNumber::Hash(hash))
                .map_err(|err| EngineApiError::Internal(Box::new(err)))?;
            result.push(block.map(convert_to_payload_body_v1));
        }

        Ok(result)
    }

    /// Called to verify network configuration parameters and ensure that Consensus and Execution
    /// layers are using the latest configuration.
    pub async fn exchange_transition_configuration(
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
            .fork(Hardfork::Paris)
            .ttd()
            .expect("the engine API should not be running for chains w/o paris");

        // Compare total difficulty values
        if merge_terminal_td != terminal_total_difficulty {
            return Err(EngineApiError::TerminalTD {
                execution: merge_terminal_td,
                consensus: terminal_total_difficulty,
            })
        }

        self.inner.beacon_consensus.transition_configuration_exchanged().await;

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
            .block_hash(terminal_block_number.to())
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

    /// Validates the timestamp depending on the version called:
    ///
    /// * If V2, this ensure that the payload timestamp is pre-Cancun.
    /// * If V3, this ensures that the payload timestamp is within the Cancun timestamp.
    ///
    /// Otherwise, this will return [EngineApiError::UnsupportedFork].
    fn validate_payload_timestamp(
        &self,
        version: EngineApiMessageVersion,
        timestamp: u64,
    ) -> EngineApiResult<()> {
        let is_cancun = self.inner.chain_spec.is_cancun_active_at_timestamp(timestamp);
        if version == EngineApiMessageVersion::V2 && is_cancun {
            // From the Engine API spec:
            //
            // ### Update the methods of previous forks
            //
            // This document defines how Cancun payload should be handled by the [`Shanghai
            // API`](https://github.com/ethereum/execution-apis/blob/ff43500e653abde45aec0f545564abfb648317af/src/engine/shanghai.md).
            //
            // For the following methods:
            //
            // - [`engine_forkchoiceUpdatedV2`](https://github.com/ethereum/execution-apis/blob/ff43500e653abde45aec0f545564abfb648317af/src/engine/shanghai.md#engine_forkchoiceupdatedv2)
            // - [`engine_newPayloadV2`](https://github.com/ethereum/execution-apis/blob/ff43500e653abde45aec0f545564abfb648317af/src/engine/shanghai.md#engine_newpayloadV2)
            // - [`engine_getPayloadV2`](https://github.com/ethereum/execution-apis/blob/ff43500e653abde45aec0f545564abfb648317af/src/engine/shanghai.md#engine_getpayloadv2)
            //
            // a validation **MUST** be added:
            //
            // 1. Client software **MUST** return `-38005: Unsupported fork` error if the
            //    `timestamp` of payload or payloadAttributes greater or equal to the Cancun
            //    activation timestamp.
            return Err(EngineApiError::UnsupportedFork)
        }

        if version == EngineApiMessageVersion::V3 && !is_cancun {
            // From the Engine API spec:
            // <https://github.com/ethereum/execution-apis/blob/ff43500e653abde45aec0f545564abfb648317af/src/engine/cancun.md#specification-2>
            //
            // 1. Client software **MUST** return `-38005: Unsupported fork` error if the
            //    `timestamp` of the built payload does not fall within the time frame of the Cancun
            //    fork.
            return Err(EngineApiError::UnsupportedFork)
        }
        Ok(())
    }

    /// Validates the presence of the `withdrawals` field according to the payload timestamp.
    /// After Shanghai, withdrawals field must be [Some].
    /// Before Shanghai, withdrawals field must be [None];
    fn validate_withdrawals_presence(
        &self,
        version: EngineApiMessageVersion,
        timestamp: u64,
        has_withdrawals: bool,
    ) -> EngineApiResult<()> {
        let is_shanghai =
            self.inner.chain_spec.fork(Hardfork::Shanghai).active_at_timestamp(timestamp);

        match version {
            EngineApiMessageVersion::V1 => {
                if has_withdrawals {
                    return Err(EngineApiError::WithdrawalsNotSupportedInV1)
                }
                if is_shanghai {
                    return Err(EngineApiError::NoWithdrawalsPostShanghai)
                }
            }
            EngineApiMessageVersion::V2 | EngineApiMessageVersion::V3 => {
                if is_shanghai && !has_withdrawals {
                    return Err(EngineApiError::NoWithdrawalsPostShanghai)
                }
                if !is_shanghai && has_withdrawals {
                    return Err(EngineApiError::HasWithdrawalsPreShanghai)
                }
            }
        };

        Ok(())
    }

    /// Validate the presence of the `parentBeaconBlockRoot` field according to the payload
    /// timestamp.
    ///
    /// After Cancun, `parentBeaconBlockRoot` field must be [Some].
    /// Before Cancun, `parentBeaconBlockRoot` field must be [None].
    ///
    /// If the engine API message version is V1 or V2, and the payload attribute's timestamp is
    /// post-Cancun, then this will return [EngineApiError::UnsupportedFork].
    ///
    /// If the payload attribute's timestamp is before the Cancun fork and the engine API message
    /// version is V3, then this will return [EngineApiError::UnsupportedFork].
    ///
    /// If the engine API message version is V3, but the `parentBeaconBlockRoot` is [None], then
    /// this will return [EngineApiError::NoParentBeaconBlockRootPostCancun].
    ///
    /// This implements the following Engine API spec rules:
    ///
    /// 1. Client software **MUST** check that provided set of parameters and their fields strictly
    ///    matches the expected one and return `-32602: Invalid params` error if this check fails.
    ///    Any field having `null` value **MUST** be considered as not provided.
    ///
    /// 2. Client software **MUST** return `-38005: Unsupported fork` error if the
    ///    `payloadAttributes` is set and the `payloadAttributes.timestamp` does not fall within the
    ///    time frame of the Cancun fork.
    fn validate_parent_beacon_block_root_presence(
        &self,
        version: EngineApiMessageVersion,
        timestamp: u64,
        has_parent_beacon_block_root: bool,
    ) -> EngineApiResult<()> {
        // 1. Client software **MUST** check that provided set of parameters and their fields
        //    strictly matches the expected one and return `-32602: Invalid params` error if this
        //    check fails. Any field having `null` value **MUST** be considered as not provided.
        match version {
            EngineApiMessageVersion::V1 | EngineApiMessageVersion::V2 => {
                if has_parent_beacon_block_root {
                    return Err(EngineApiError::ParentBeaconBlockRootNotSupportedBeforeV3)
                }
            }
            EngineApiMessageVersion::V3 => {
                if !has_parent_beacon_block_root {
                    return Err(EngineApiError::NoParentBeaconBlockRootPostCancun)
                }
            }
        };

        // 2. Client software **MUST** return `-38005: Unsupported fork` error if the
        //    `payloadAttributes` is set and the `payloadAttributes.timestamp` does not fall within
        //    the time frame of the Cancun fork.
        self.validate_payload_timestamp(version, timestamp)?;

        Ok(())
    }

    /// Validates the presence or exclusion of fork-specific fields based on the payload attributes
    /// and the message version.
    fn validate_version_specific_fields<Type>(
        &self,
        version: EngineApiMessageVersion,
        payload_or_attrs: &PayloadOrAttributes<'_, Type>,
    ) -> EngineApiResult<()>
    where
        Type: PayloadAttributesTrait,
    {
        self.validate_withdrawals_presence(
            version,
            payload_or_attrs.timestamp(),
            payload_or_attrs.withdrawals().is_some(),
        )?;
        self.validate_parent_beacon_block_root_presence(
            version,
            payload_or_attrs.timestamp(),
            payload_or_attrs.parent_beacon_block_root().is_some(),
        )
    }

    /// Validates the `engine_forkchoiceUpdated` payload attributes and executes the forkchoice
    /// update.
    ///
    /// The payload attributes will be validated according to the engine API rules for the given
    /// message version:
    /// * If the version is [EngineApiMessageVersion::V1], then the payload attributes will be
    ///   validated according to the Paris rules.
    /// * If the version is [EngineApiMessageVersion::V2], then the payload attributes will be
    ///   validated according to the Shanghai rules, as well as the validity changes from cancun:
    ///   <https://github.com/ethereum/execution-apis/blob/584905270d8ad665718058060267061ecfd79ca5/src/engine/cancun.md#update-the-methods-of-previous-forks>
    ///
    /// * If the version is [EngineApiMessageVersion::V3], then the payload attributes will be
    ///   validated according to the Cancun rules.
    async fn validate_and_execute_forkchoice(
        &self,
        version: EngineApiMessageVersion,
        state: ForkchoiceState,
        payload_attrs: Option<Types::PayloadAttributes>,
    ) -> EngineApiResult<ForkchoiceUpdated> {
        if let Some(ref attrs) = payload_attrs {
            let attr_validation_res = self.validate_version_specific_fields(version, &attrs.into());

            #[cfg(feature = "optimism")]
            if attrs.optimism_payload_attributes.gas_limit.is_none() &&
                self.inner.chain_spec.is_optimism()
            {
                return Err(EngineApiError::MissingGasLimitInPayloadAttributes)
            }

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
                return Err(err)
            }
        }

        Ok(self.inner.beacon_consensus.fork_choice_updated(state, payload_attrs).await?)
    }
}

#[async_trait]
impl<Provider, Types> EngineApiServer<Types> for EngineApi<Provider, Types>
where
    Provider: HeaderProvider + BlockReader + StateProviderFactory + EvmEnvProvider + 'static,
    Types: EngineTypes + 'static + Send,
    Types::PayloadAttributes: Send,
    Types::PayloadBuilderAttributes: Send,
{
    /// Handler for `engine_newPayloadV1`
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/paris.md#engine_newpayloadv1>
    /// Caution: This should not accept the `withdrawals` field
    async fn new_payload_v1(&self, payload: ExecutionPayloadV1) -> RpcResult<PayloadStatus> {
        trace!(target: "rpc::engine", "Serving engine_newPayloadV1");
        let start = Instant::now();
        let res = EngineApi::new_payload_v1(self, payload).await;
        self.inner.metrics.new_payload_v1.record(start.elapsed());
        Ok(res?)
    }

    /// Handler for `engine_newPayloadV2`
    /// See also <https://github.com/ethereum/execution-apis/blob/584905270d8ad665718058060267061ecfd79ca5/src/engine/shanghai.md#engine_newpayloadv2>
    async fn new_payload_v2(&self, payload: ExecutionPayloadInputV2) -> RpcResult<PayloadStatus> {
        trace!(target: "rpc::engine", "Serving engine_newPayloadV2");
        let start = Instant::now();
        let res = EngineApi::new_payload_v2(self, payload).await;
        self.inner.metrics.new_payload_v2.record(start.elapsed());
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
        let res =
            EngineApi::new_payload_v3(self, payload, versioned_hashes, parent_beacon_block_root)
                .await;
        self.inner.metrics.new_payload_v3.record(start.elapsed());
        Ok(res?)
    }

    /// Handler for `engine_forkchoiceUpdatedV1`
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/paris.md#engine_forkchoiceupdatedv1>
    ///
    /// Caution: This should not accept the `withdrawals` field
    async fn fork_choice_updated_v1(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<Types::PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        trace!(target: "rpc::engine", "Serving engine_forkchoiceUpdatedV1");
        let start = Instant::now();
        let res =
            EngineApi::fork_choice_updated_v1(self, fork_choice_state, payload_attributes).await;
        self.inner.metrics.fork_choice_updated_v1.record(start.elapsed());
        Ok(res?)
    }

    /// Handler for `engine_forkchoiceUpdatedV2`
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/shanghai.md#engine_forkchoiceupdatedv2>
    async fn fork_choice_updated_v2(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<Types::PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        trace!(target: "rpc::engine", "Serving engine_forkchoiceUpdatedV2");
        let start = Instant::now();
        let res =
            EngineApi::fork_choice_updated_v2(self, fork_choice_state, payload_attributes).await;
        self.inner.metrics.fork_choice_updated_v2.record(start.elapsed());
        Ok(res?)
    }

    /// Handler for `engine_forkchoiceUpdatedV2`
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/cancun.md#engine_forkchoiceupdatedv3>
    async fn fork_choice_updated_v3(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<Types::PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        trace!(target: "rpc::engine", "Serving engine_forkchoiceUpdatedV3");
        let start = Instant::now();
        let res =
            EngineApi::fork_choice_updated_v3(self, fork_choice_state, payload_attributes).await;
        self.inner.metrics.fork_choice_updated_v3.record(start.elapsed());
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
    async fn get_payload_v1(&self, payload_id: PayloadId) -> RpcResult<ExecutionPayloadV1> {
        trace!(target: "rpc::engine", "Serving engine_getPayloadV1");
        let start = Instant::now();
        let res = EngineApi::get_payload_v1(self, payload_id).await;
        self.inner.metrics.get_payload_v1.record(start.elapsed());
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
    async fn get_payload_v2(&self, payload_id: PayloadId) -> RpcResult<ExecutionPayloadEnvelopeV2> {
        trace!(target: "rpc::engine", "Serving engine_getPayloadV2");
        let start = Instant::now();
        let res = EngineApi::get_payload_v2(self, payload_id).await;
        self.inner.metrics.get_payload_v2.record(start.elapsed());
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
    async fn get_payload_v3(&self, payload_id: PayloadId) -> RpcResult<ExecutionPayloadEnvelopeV3> {
        trace!(target: "rpc::engine", "Serving engine_getPayloadV3");
        let start = Instant::now();
        let res = EngineApi::get_payload_v3(self, payload_id).await;
        self.inner.metrics.get_payload_v3.record(start.elapsed());
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
        let res = EngineApi::get_payload_bodies_by_hash(self, block_hashes);
        self.inner.metrics.get_payload_bodies_by_hash_v1.record(start.elapsed());
        Ok(res?)
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
    /// Implementors should take care when acting on the input to this method, specifically
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
        let res = EngineApi::get_payload_bodies_by_range(self, start.to(), count.to()).await;
        self.inner.metrics.get_payload_bodies_by_range_v1.record(start_time.elapsed());
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
        let res = EngineApi::exchange_transition_configuration(self, config).await;
        self.inner.metrics.exchange_transition_configuration.record(start.elapsed());
        Ok(res?)
    }

    /// Handler for `engine_exchangeCapabilitiesV1`
    /// See also <https://github.com/ethereum/execution-apis/blob/6452a6b194d7db269bf1dbd087a267251d3cc7f8/src/engine/common.md#capabilities>
    async fn exchange_capabilities(&self, _capabilities: Vec<String>) -> RpcResult<Vec<String>> {
        Ok(CAPABILITIES.into_iter().map(str::to_owned).collect())
    }
}

impl<Provider, Types> std::fmt::Debug for EngineApi<Provider, Types>
where
    Types: EngineTypes,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineApi").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use reth_beacon_consensus::BeaconEngineMessage;
    use reth_interfaces::test_utils::generators::random_block;
    use reth_payload_builder::{test_utils::spawn_test_payload_service, EthEngineTypes};
    use reth_primitives::{SealedBlock, B256, MAINNET};
    use reth_provider::test_utils::MockEthProvider;
    use reth_rpc_types_compat::engine::payload::execution_payload_from_sealed_block;
    use reth_tasks::TokioTaskExecutor;
    use std::sync::Arc;
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

    fn setup_engine_api() -> (EngineApiTestHandle, EngineApi<Arc<MockEthProvider>, EthEngineTypes>)
    {
        let chain_spec: Arc<ChainSpec> = MAINNET.clone();
        let provider = Arc::new(MockEthProvider::default());
        let payload_store = spawn_test_payload_service();
        let (to_engine, engine_rx) = unbounded_channel();
        let task_executor = Box::<TokioTaskExecutor>::default();
        let api = EngineApi::new(
            provider.clone(),
            chain_spec.clone(),
            BeaconConsensusEngineHandle::new(to_engine),
            payload_store.into(),
            task_executor,
        );
        let handle = EngineApiTestHandle { chain_spec, provider, from_api: engine_rx };
        (handle, api)
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
        use reth_interfaces::test_utils::{generators, generators::random_block_range};

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
                let res = api.get_payload_bodies_by_range(start, count).await;
                assert_matches!(res, Err(EngineApiError::InvalidBodiesRange { .. }));
            }
        }

        #[tokio::test]
        async fn request_too_large() {
            let (_, api) = setup_engine_api();

            let request_count = MAX_PAYLOAD_BODIES_LIMIT + 1;
            let res = api.get_payload_bodies_by_range(0, request_count).await;
            assert_matches!(res, Err(EngineApiError::PayloadRequestTooLarge { .. }));
        }

        #[tokio::test]
        async fn returns_payload_bodies() {
            let mut rng = generators::rng();
            let (handle, api) = setup_engine_api();

            let (start, count) = (1, 10);
            let blocks =
                random_block_range(&mut rng, start..=start + count - 1, B256::default(), 0..2);
            handle.provider.extend_blocks(blocks.iter().cloned().map(|b| (b.hash(), b.unseal())));

            let expected = blocks
                .iter()
                .cloned()
                .map(|b| Some(convert_to_payload_body_v1(b.unseal())))
                .collect::<Vec<_>>();

            let res = api.get_payload_bodies_by_range(start, count).await.unwrap();
            assert_eq!(res, expected);
        }

        #[tokio::test]
        async fn returns_payload_bodies_with_gaps() {
            let mut rng = generators::rng();
            let (handle, api) = setup_engine_api();

            let (start, count) = (1, 100);
            let blocks =
                random_block_range(&mut rng, start..=start + count - 1, B256::default(), 0..2);

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

            let res = api.get_payload_bodies_by_range(start, count).await.unwrap();
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
            let res = api.get_payload_bodies_by_hash(hashes).unwrap();
            assert_eq!(res, expected);
        }
    }

    // https://github.com/ethereum/execution-apis/blob/main/src/engine/paris.md#specification-3
    mod exchange_transition_configuration {
        use super::*;
        use reth_interfaces::test_utils::generators;
        use reth_primitives::U256;

        #[tokio::test]
        async fn terminal_td_mismatch() {
            let (handle, api) = setup_engine_api();

            let transition_config = TransitionConfiguration {
                terminal_total_difficulty: handle.chain_spec.fork(Hardfork::Paris).ttd().unwrap() +
                    U256::from(1),
                ..Default::default()
            };

            let res = api.exchange_transition_configuration(transition_config.clone()).await;

            assert_matches!(
                res,
                Err(EngineApiError::TerminalTD { execution, consensus })
                    if execution == handle.chain_spec.fork(Hardfork::Paris).ttd().unwrap() && consensus == U256::from(transition_config.terminal_total_difficulty)
            );
        }

        #[tokio::test]
        async fn terminal_block_hash_mismatch() {
            let mut rng = generators::rng();

            let (handle, api) = setup_engine_api();

            let terminal_block_number = 1000;
            let consensus_terminal_block =
                random_block(&mut rng, terminal_block_number, None, None, None);
            let execution_terminal_block =
                random_block(&mut rng, terminal_block_number, None, None, None);

            let transition_config = TransitionConfiguration {
                terminal_total_difficulty: handle.chain_spec.fork(Hardfork::Paris).ttd().unwrap(),
                terminal_block_hash: consensus_terminal_block.hash(),
                terminal_block_number: U64::from(terminal_block_number),
            };

            // Unknown block number
            let res = api.exchange_transition_configuration(transition_config.clone()).await;

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

            let res = api.exchange_transition_configuration(transition_config.clone()).await;

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
                random_block(&mut generators::rng(), terminal_block_number, None, None, None);

            let transition_config = TransitionConfiguration {
                terminal_total_difficulty: handle.chain_spec.fork(Hardfork::Paris).ttd().unwrap(),
                terminal_block_hash: terminal_block.hash(),
                terminal_block_number: U64::from(terminal_block_number),
            };

            handle.provider.add_block(terminal_block.hash(), terminal_block.unseal());

            let config =
                api.exchange_transition_configuration(transition_config.clone()).await.unwrap();
            assert_eq!(config, transition_config);
        }
    }
}

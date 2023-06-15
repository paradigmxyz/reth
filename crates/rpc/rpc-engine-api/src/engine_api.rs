use crate::{EngineApiError, EngineApiMessageVersion, EngineApiResult};
use async_trait::async_trait;
use jsonrpsee_core::RpcResult;
use reth_beacon_consensus::BeaconConsensusEngineHandle;
use reth_interfaces::consensus::ForkchoiceState;
use reth_payload_builder::PayloadStore;
use reth_primitives::{BlockHash, BlockHashOrNumber, BlockNumber, ChainSpec, Hardfork, U64};
use reth_provider::{BlockProvider, EvmEnvProvider, HeaderProvider, StateProviderFactory};
use reth_rpc_api::EngineApiServer;
use reth_rpc_types::engine::{
    ExecutionPayload, ExecutionPayloadBodies, ExecutionPayloadEnvelope, ForkchoiceUpdated,
    PayloadAttributes, PayloadId, PayloadStatus, TransitionConfiguration, CAPABILITIES,
};
use std::sync::Arc;
use tokio::sync::oneshot;
use tracing::trace;

/// The Engine API response sender.
pub type EngineApiSender<Ok> = oneshot::Sender<EngineApiResult<Ok>>;

/// The upper limit for payload bodies request.
const MAX_PAYLOAD_BODIES_LIMIT: u64 = 1024;

/// The Engine API implementation that grants the Consensus layer access to data and
/// functions in the Execution layer that are crucial for the consensus process.
pub struct EngineApi<Provider> {
    /// The provider to interact with the chain.
    provider: Provider,
    /// Consensus configuration
    chain_spec: Arc<ChainSpec>,
    /// The channel to send messages to the beacon consensus engine.
    beacon_consensus: BeaconConsensusEngineHandle,
    /// The type that can communicate with the payload service to retrieve payloads.
    payload_store: PayloadStore,
}

impl<Provider> EngineApi<Provider>
where
    Provider: HeaderProvider + BlockProvider + StateProviderFactory + EvmEnvProvider + 'static,
{
    /// Create new instance of [EngineApi].
    pub fn new(
        provider: Provider,
        chain_spec: Arc<ChainSpec>,
        beacon_consensus: BeaconConsensusEngineHandle,
        payload_store: PayloadStore,
    ) -> Self {
        Self { provider, chain_spec, beacon_consensus, payload_store }
    }

    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/paris.md#engine_newpayloadv1>
    /// Caution: This should not accept the `withdrawals` field
    pub async fn new_payload_v1(
        &self,
        payload: ExecutionPayload,
    ) -> EngineApiResult<PayloadStatus> {
        self.validate_withdrawals_presence(
            EngineApiMessageVersion::V1,
            payload.timestamp.as_u64(),
            payload.withdrawals.is_some(),
        )?;
        Ok(self.beacon_consensus.new_payload(payload).await?)
    }

    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/shanghai.md#engine_newpayloadv2>
    pub async fn new_payload_v2(
        &self,
        payload: ExecutionPayload,
    ) -> EngineApiResult<PayloadStatus> {
        self.validate_withdrawals_presence(
            EngineApiMessageVersion::V2,
            payload.timestamp.as_u64(),
            payload.withdrawals.is_some(),
        )?;
        Ok(self.beacon_consensus.new_payload(payload).await?)
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
        payload_attrs: Option<PayloadAttributes>,
    ) -> EngineApiResult<ForkchoiceUpdated> {
        if let Some(ref attrs) = payload_attrs {
            self.validate_withdrawals_presence(
                EngineApiMessageVersion::V1,
                attrs.timestamp.as_u64(),
                attrs.withdrawals.is_some(),
            )?;
        }
        Ok(self.beacon_consensus.fork_choice_updated(state, payload_attrs).await?)
    }

    /// Sends a message to the beacon consensus engine to update the fork choice _with_ withdrawals,
    /// but only _after_ shanghai.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/shanghai.md#engine_forkchoiceupdatedv2>
    pub async fn fork_choice_updated_v2(
        &self,
        state: ForkchoiceState,
        payload_attrs: Option<PayloadAttributes>,
    ) -> EngineApiResult<ForkchoiceUpdated> {
        if let Some(ref attrs) = payload_attrs {
            self.validate_withdrawals_presence(
                EngineApiMessageVersion::V2,
                attrs.timestamp.as_u64(),
                attrs.withdrawals.is_some(),
            )?;
        }
        Ok(self.beacon_consensus.fork_choice_updated(state, payload_attrs).await?)
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
    pub async fn get_payload_v1(&self, payload_id: PayloadId) -> EngineApiResult<ExecutionPayload> {
        Ok(self
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
    async fn get_payload_v2(
        &self,
        payload_id: PayloadId,
    ) -> EngineApiResult<ExecutionPayloadEnvelope> {
        Ok(self
            .payload_store
            .resolve(payload_id)
            .await
            .ok_or(EngineApiError::UnknownPayload)?
            .map(|payload| (*payload).clone().into_v2_payload())?)
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
    pub fn get_payload_bodies_by_range(
        &self,
        start: BlockNumber,
        count: u64,
    ) -> EngineApiResult<ExecutionPayloadBodies> {
        if count > MAX_PAYLOAD_BODIES_LIMIT {
            return Err(EngineApiError::PayloadRequestTooLarge { len: count })
        }

        if start == 0 || count == 0 {
            return Err(EngineApiError::InvalidBodiesRange { start, count })
        }

        let mut result = Vec::with_capacity(count as usize);

        let end = start.saturating_add(count);
        for num in start..end {
            let block = self
                .provider
                .block(BlockHashOrNumber::Number(num))
                .map_err(|err| EngineApiError::Internal(Box::new(err)))?;
            result.push(block.map(Into::into));
        }

        Ok(result)
    }

    /// Called to retrieve execution payload bodies by hashes.
    pub fn get_payload_bodies_by_hash(
        &self,
        hashes: Vec<BlockHash>,
    ) -> EngineApiResult<ExecutionPayloadBodies> {
        let len = hashes.len() as u64;
        if len > MAX_PAYLOAD_BODIES_LIMIT {
            return Err(EngineApiError::PayloadRequestTooLarge { len })
        }

        let mut result = Vec::with_capacity(hashes.len());
        for hash in hashes {
            let block = self
                .provider
                .block(BlockHashOrNumber::Hash(hash))
                .map_err(|err| EngineApiError::Internal(Box::new(err)))?;
            result.push(block.map(Into::into));
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

        self.beacon_consensus.transition_configuration_exchanged().await;

        // Short circuit if communicated block hash is zero
        if terminal_block_hash.is_zero() {
            return Ok(TransitionConfiguration {
                terminal_total_difficulty: merge_terminal_td,
                ..Default::default()
            })
        }

        // Attempt to look up terminal block hash
        let local_hash = self
            .provider
            .block_hash(terminal_block_number.as_u64())
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

    /// Validates the presence of the `withdrawals` field according to the payload timestamp.
    /// After Shanghai, withdrawals field must be [Some].
    /// Before Shanghai, withdrawals field must be [None];
    fn validate_withdrawals_presence(
        &self,
        version: EngineApiMessageVersion,
        timestamp: u64,
        has_withdrawals: bool,
    ) -> EngineApiResult<()> {
        let is_shanghai = self.chain_spec.fork(Hardfork::Shanghai).active_at_timestamp(timestamp);

        match version {
            EngineApiMessageVersion::V1 => {
                if has_withdrawals {
                    return Err(EngineApiError::WithdrawalsNotSupportedInV1)
                }
                if is_shanghai {
                    return Err(EngineApiError::NoWithdrawalsPostShanghai)
                }
            }
            EngineApiMessageVersion::V2 => {
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
}

#[async_trait]
impl<Provider> EngineApiServer for EngineApi<Provider>
where
    Provider: HeaderProvider + BlockProvider + StateProviderFactory + EvmEnvProvider + 'static,
{
    /// Handler for `engine_newPayloadV1`
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/paris.md#engine_newpayloadv1>
    /// Caution: This should not accept the `withdrawals` field
    async fn new_payload_v1(&self, payload: ExecutionPayload) -> RpcResult<PayloadStatus> {
        trace!(target: "rpc::engine", "Serving engine_newPayloadV1");
        Ok(EngineApi::new_payload_v1(self, payload).await?)
    }

    /// Handler for `engine_newPayloadV1`
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/shanghai.md#engine_newpayloadv2>
    async fn new_payload_v2(&self, payload: ExecutionPayload) -> RpcResult<PayloadStatus> {
        trace!(target: "rpc::engine", "Serving engine_newPayloadV1");
        Ok(EngineApi::new_payload_v2(self, payload).await?)
    }

    /// Handler for `engine_forkchoiceUpdatedV1`
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/paris.md#engine_forkchoiceupdatedv1>
    ///
    /// Caution: This should not accept the `withdrawals` field
    async fn fork_choice_updated_v1(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        trace!(target: "rpc::engine", "Serving engine_forkchoiceUpdatedV1");
        Ok(EngineApi::fork_choice_updated_v1(self, fork_choice_state, payload_attributes).await?)
    }

    /// Handler for `engine_forkchoiceUpdatedV2`
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/shanghai.md#engine_forkchoiceupdatedv2>
    async fn fork_choice_updated_v2(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        trace!(target: "rpc::engine", "Serving engine_forkchoiceUpdatedV2");
        Ok(EngineApi::fork_choice_updated_v2(self, fork_choice_state, payload_attributes).await?)
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
    async fn get_payload_v1(&self, payload_id: PayloadId) -> RpcResult<ExecutionPayload> {
        trace!(target: "rpc::engine", "Serving engine_getPayloadV1");
        Ok(EngineApi::get_payload_v1(self, payload_id).await?)
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
    async fn get_payload_v2(&self, payload_id: PayloadId) -> RpcResult<ExecutionPayloadEnvelope> {
        trace!(target: "rpc::engine", "Serving engine_getPayloadV2");
        Ok(EngineApi::get_payload_v2(self, payload_id).await?)
    }

    /// Handler for `engine_getPayloadBodiesByHashV1`
    /// See also <https://github.com/ethereum/execution-apis/blob/6452a6b194d7db269bf1dbd087a267251d3cc7f8/src/engine/shanghai.md#engine_getpayloadbodiesbyhashv1>
    async fn get_payload_bodies_by_hash_v1(
        &self,
        block_hashes: Vec<BlockHash>,
    ) -> RpcResult<ExecutionPayloadBodies> {
        trace!(target: "rpc::engine", "Serving engine_getPayloadBodiesByHashV1");
        Ok(EngineApi::get_payload_bodies_by_hash(self, block_hashes)?)
    }

    /// Handler for `engine_getPayloadBodiesByRangeV1`
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
    async fn get_payload_bodies_by_range_v1(
        &self,
        start: U64,
        count: U64,
    ) -> RpcResult<ExecutionPayloadBodies> {
        trace!(target: "rpc::engine", "Serving engine_getPayloadBodiesByRangeV1");
        Ok(EngineApi::get_payload_bodies_by_range(self, start.as_u64(), count.as_u64())?)
    }

    /// Handler for `engine_exchangeTransitionConfigurationV1`
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/paris.md#engine_exchangeTransitionConfigurationV1>
    async fn exchange_transition_configuration(
        &self,
        config: TransitionConfiguration,
    ) -> RpcResult<TransitionConfiguration> {
        trace!(target: "rpc::engine", "Serving engine_exchangeTransitionConfigurationV1");
        Ok(EngineApi::exchange_transition_configuration(self, config).await?)
    }

    /// Handler for `engine_exchangeCapabilitiesV1`
    /// See also <https://github.com/ethereum/execution-apis/blob/6452a6b194d7db269bf1dbd087a267251d3cc7f8/src/engine/common.md#capabilities>
    async fn exchange_capabilities(&self, _capabilities: Vec<String>) -> RpcResult<Vec<String>> {
        Ok(CAPABILITIES.into_iter().map(str::to_owned).collect())
    }
}

impl<Provider> std::fmt::Debug for EngineApi<Provider> {
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
    use reth_payload_builder::test_utils::spawn_test_payload_service;
    use reth_primitives::{SealedBlock, H256, MAINNET};
    use reth_provider::test_utils::MockEthProvider;
    use std::sync::Arc;
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

    fn setup_engine_api() -> (EngineApiTestHandle, EngineApi<Arc<MockEthProvider>>) {
        let chain_spec: Arc<ChainSpec> = MAINNET.clone();
        let provider = Arc::new(MockEthProvider::default());
        let payload_store = spawn_test_payload_service();
        let (to_engine, engine_rx) = unbounded_channel();
        let api = EngineApi::new(
            provider.clone(),
            chain_spec.clone(),
            BeaconConsensusEngineHandle::new(to_engine),
            payload_store.into(),
        );
        let handle = EngineApiTestHandle { chain_spec, provider, from_api: engine_rx };
        (handle, api)
    }

    struct EngineApiTestHandle {
        chain_spec: Arc<ChainSpec>,
        provider: Arc<MockEthProvider>,
        from_api: UnboundedReceiver<BeaconEngineMessage>,
    }

    #[tokio::test]
    async fn forwards_responses_to_consensus_engine() {
        let (mut handle, api) = setup_engine_api();

        tokio::spawn(async move {
            api.new_payload_v1(SealedBlock::default().into()).await.unwrap();
        });
        assert_matches!(handle.from_api.recv().await, Some(BeaconEngineMessage::NewPayload { .. }));
    }

    // tests covering `engine_getPayloadBodiesByRange` and `engine_getPayloadBodiesByHash`
    mod get_payload_bodies {
        use super::*;
        use reth_interfaces::test_utils::generators::random_block_range;

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
                let res = api.get_payload_bodies_by_range(start, count);
                assert_matches!(res, Err(EngineApiError::InvalidBodiesRange { .. }));
            }
        }

        #[tokio::test]
        async fn request_too_large() {
            let (_, api) = setup_engine_api();

            let request_count = MAX_PAYLOAD_BODIES_LIMIT + 1;
            let res = api.get_payload_bodies_by_range(0, request_count);
            assert_matches!(res, Err(EngineApiError::PayloadRequestTooLarge { .. }));
        }

        #[tokio::test]
        async fn returns_payload_bodies() {
            let (handle, api) = setup_engine_api();

            let (start, count) = (1, 10);
            let blocks = random_block_range(start..=start + count - 1, H256::default(), 0..2);
            handle.provider.extend_blocks(blocks.iter().cloned().map(|b| (b.hash(), b.unseal())));

            let expected =
                blocks.iter().cloned().map(|b| Some(b.unseal().into())).collect::<Vec<_>>();

            let res = api.get_payload_bodies_by_range(start, count).unwrap();
            assert_eq!(res, expected);
        }

        #[tokio::test]
        async fn returns_payload_bodies_with_gaps() {
            let (handle, api) = setup_engine_api();

            let (start, count) = (1, 100);
            let blocks = random_block_range(start..=start + count - 1, H256::default(), 0..2);

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
                .cloned()
                .map(|b| {
                    if first_missing_range.contains(&b.number) ||
                        second_missing_range.contains(&b.number)
                    {
                        None
                    } else {
                        Some(b.unseal().into())
                    }
                })
                .collect::<Vec<_>>();

            let res = api.get_payload_bodies_by_range(start, count).unwrap();
            assert_eq!(res, expected);

            let hashes = blocks.iter().map(|b| b.hash()).collect();
            let res = api.get_payload_bodies_by_hash(hashes).unwrap();
            assert_eq!(res, expected);
        }
    }

    // https://github.com/ethereum/execution-apis/blob/main/src/engine/paris.md#specification-3
    mod exchange_transition_configuration {
        use super::*;
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
            let (handle, api) = setup_engine_api();

            let terminal_block_number = 1000;
            let consensus_terminal_block = random_block(terminal_block_number, None, None, None);
            let execution_terminal_block = random_block(terminal_block_number, None, None, None);

            let transition_config = TransitionConfiguration {
                terminal_total_difficulty: handle.chain_spec.fork(Hardfork::Paris).ttd().unwrap(),
                terminal_block_hash: consensus_terminal_block.hash(),
                terminal_block_number: terminal_block_number.into(),
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
            let terminal_block = random_block(terminal_block_number, None, None, None);

            let transition_config = TransitionConfiguration {
                terminal_total_difficulty: handle.chain_spec.fork(Hardfork::Paris).ttd().unwrap(),
                terminal_block_hash: terminal_block.hash(),
                terminal_block_number: terminal_block_number.into(),
            };

            handle.provider.add_block(terminal_block.hash(), terminal_block.unseal());

            let config =
                api.exchange_transition_configuration(transition_config.clone()).await.unwrap();
            assert_eq!(config, transition_config);
        }
    }
}

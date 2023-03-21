use crate::{message::EngineApiMessageVersion, EngineApiError, EngineApiMessage, EngineApiResult};
use futures::StreamExt;
use reth_beacon_consensus::BeaconEngineMessage;
use reth_interfaces::consensus::ForkchoiceState;
use reth_primitives::{
    BlockHash, BlockId, BlockNumber, ChainSpec, Hardfork, SealedBlock, H256, H64, U256,
};
use reth_provider::{BlockProvider, EvmEnvProvider, HeaderProvider, StateProviderFactory};
use reth_rpc_types::engine::{
    ExecutionPayload, ExecutionPayloadBodies, ForkchoiceUpdated, PayloadAttributes, PayloadStatus,
    PayloadStatusEnum, TransitionConfiguration,
};
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::sync::{
    mpsc::{self, UnboundedSender},
    oneshot,
};
use tokio_stream::wrappers::UnboundedReceiverStream;

/// The Engine API handle.
pub type EngineApiHandle = mpsc::UnboundedSender<EngineApiMessage>;

/// The Engine API response sender.
pub type EngineApiSender<Ok> = oneshot::Sender<EngineApiResult<Ok>>;

/// The upper limit for payload bodies request.
const MAX_PAYLOAD_BODIES_LIMIT: u64 = 1024;

/// The Engine API implementation that grants the Consensus layer access to data and
/// functions in the Execution layer that are crucial for the consensus process.
#[must_use = "EngineApi does nothing unless polled."]
pub struct EngineApi<Client> {
    client: Client,
    /// Consensus configuration
    chain_spec: Arc<ChainSpec>,
    message_rx: UnboundedReceiverStream<EngineApiMessage>,
    engine_tx: UnboundedSender<BeaconEngineMessage>,
}

impl<Client: HeaderProvider + BlockProvider + StateProviderFactory + EvmEnvProvider>
    EngineApi<Client>
{
    /// Create new instance of [EngineApi].
    pub fn new(
        client: Client,
        chain_spec: Arc<ChainSpec>,
        message_rx: mpsc::UnboundedReceiver<EngineApiMessage>,
        engine_tx: UnboundedSender<BeaconEngineMessage>,
    ) -> Self {
        Self { client, chain_spec, message_rx: UnboundedReceiverStream::new(message_rx), engine_tx }
    }

    fn on_message(&mut self, msg: EngineApiMessage) {
        match msg {
            EngineApiMessage::ExchangeTransitionConfiguration(config, tx) => {
                let _ = tx.send(self.exchange_transition_configuration(config));
            }
            EngineApiMessage::GetPayload(payload_id, tx) => {
                let _ = tx.send(self.get_payload(payload_id).ok_or(EngineApiError::PayloadUnknown));
            }
            EngineApiMessage::GetPayloadBodiesByHash(hashes, tx) => {
                let _ = tx.send(self.get_payload_bodies_by_hash(hashes));
            }
            EngineApiMessage::GetPayloadBodiesByRange(start, count, tx) => {
                let _ = tx.send(self.get_payload_bodies_by_range(start, count));
            }
            EngineApiMessage::NewPayload(version, payload, tx) => {
                if let Err(error) = self.validate_withdrawals_presence(
                    version,
                    payload.timestamp.as_u64(),
                    payload.withdrawals.is_some(),
                ) {
                    let _ = tx.send(Ok(PayloadStatus::from_status(PayloadStatusEnum::Invalid {
                        validation_error: error.to_string(),
                    })));
                    return
                }

                let block: SealedBlock = match payload.try_into() {
                    Ok(block) => block,
                    Err(error) => {
                        let response =
                            PayloadStatus::from_status(PayloadStatusEnum::InvalidBlockHash {
                                validation_error: error.to_string(),
                            });
                        let _ = tx.send(Ok(response));
                        return
                    }
                };

                // forward message to the consensus engine
                let _ = self.engine_tx.send(BeaconEngineMessage::NewPayload(block, tx));
            }
            EngineApiMessage::ForkchoiceUpdated(version, state, attrs, tx) => {
                if let Some(attributes) = &attrs {
                    if let Err(error) = self.validate_withdrawals_presence(
                        version,
                        attributes.timestamp.as_u64(),
                        attributes.withdrawals.is_some(),
                    ) {
                        let _ = tx.send(Ok(ForkchoiceUpdated::from_status(
                            PayloadStatusEnum::Invalid { validation_error: error.to_string() },
                        )));
                        return
                    }
                }

                let _ =
                    self.engine_tx.send(BeaconEngineMessage::ForkchoiceUpdated(state, attrs, tx));
            }
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
                if is_shanghai || has_withdrawals {
                    return Err(EngineApiError::InvalidParams)
                }
            }
            EngineApiMessageVersion::V2 => {
                let shanghai_with_no_withdrawals = is_shanghai && !has_withdrawals;
                let not_shanghai_with_withdrawals = !is_shanghai && has_withdrawals;
                if shanghai_with_no_withdrawals || not_shanghai_with_withdrawals {
                    return Err(EngineApiError::InvalidParams)
                }
            }
        };

        Ok(())
    }

    /// Called to retrieve the latest state of the network, validate new blocks, and maintain
    /// consistency between the Consensus and Execution layers.
    ///
    /// NOTE: Will always result in `PayloadUnknown` since we don't support block
    /// building for now.
    pub fn get_payload(&self, _payload_id: H64) -> Option<ExecutionPayload> {
        None
    }

    /// Called to retrieve execution payload bodies by range.
    pub fn get_payload_bodies_by_range(
        &self,
        start: BlockNumber,
        count: u64,
    ) -> EngineApiResult<ExecutionPayloadBodies> {
        if count > MAX_PAYLOAD_BODIES_LIMIT {
            return Err(EngineApiError::PayloadRequestTooLarge { len: count })
        }

        if start == 0 || count == 0 {
            return Err(EngineApiError::InvalidParams)
        }

        let mut result = Vec::with_capacity(count as usize);
        for num in start..start + count {
            let block = self.client.block(BlockId::Number(num.into()))?;
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
            let block = self.client.block(BlockId::Hash(hash.into()))?;
            result.push(block.map(Into::into));
        }

        Ok(result)
    }

    /// Called to resolve chain forks and ensure that the Execution layer is working with the latest
    /// valid chain.
    ///
    /// These responses should adhere to the [Engine API Spec for
    /// `engine_forkchoiceUpdated`](https://github.com/ethereum/execution-apis/blob/main/src/engine/paris.md#specification-1).
    pub fn fork_choice_updated(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> EngineApiResult<ForkchoiceUpdated> {
        let ForkchoiceState { head_block_hash, finalized_block_hash, .. } = fork_choice_state;

        if head_block_hash.is_zero() {
            return Ok(ForkchoiceUpdated::from_status(PayloadStatusEnum::Invalid {
                validation_error: EngineApiError::ForkchoiceEmptyHead.to_string(),
            }))
        }

        // TODO: replace everything below

        let Some(head) = self.client.header(&head_block_hash)? else {
            // Block is not known, nothing to do
            return Ok(ForkchoiceUpdated::from_status(PayloadStatusEnum::Syncing))
        };

        // The finalized block hash is not known, we are still syncing
        if !finalized_block_hash.is_zero() && !self.client.is_known(&finalized_block_hash)? {
            return Ok(ForkchoiceUpdated::from_status(PayloadStatusEnum::Syncing))
        }

        let Some(head_td) = self.client.header_td(&head_block_hash)? else {
            // internal error - we have the head block but not the total difficulty
            return Ok(ForkchoiceUpdated::from_status(PayloadStatusEnum::Invalid {
                validation_error: EngineApiError::Internal(
                    reth_interfaces::provider::ProviderError::TotalDifficulty {
                        number: head.number,
                    }
                    .into(),
                )
                .to_string(),
            }))
        };

        // From the Engine API spec:
        //
        // If forkchoiceState.headBlockHash references a PoW block, client software MUST validate
        // this block with respect to terminal block conditions according to EIP-3675. This check
        // maps to the transition block validity section of the EIP. Additionally, if this
        // validation fails, client software MUST NOT update the forkchoice state and MUST NOT
        // begin a payload build process.
        //
        // We use ZERO here because as long as the total difficulty is above the ttd, we are sure
        // that the block is EITHER:
        //  * The terminal PoW block, or
        //  * A child of the terminal PoW block
        //
        // Using the head.difficulty instead of U256::ZERO here would be incorrect because it would
        // not return true on the terminal PoW block. For the terminal PoW block, head_td -
        // head.difficulty would be less than the TTD, causing active_at_ttd to return false.
        if !self.chain_spec.fork(Hardfork::Paris).active_at_ttd(head_td, U256::ZERO) {
            // This case returns a `latestValidHash` of zero because it is required by the engine
            // api spec:
            //
            // Client software MUST respond to this method call in the following way:
            // {
            //     status: INVALID,
            //     latestValidHash:
            // 0x0000000000000000000000000000000000000000000000000000000000000000,
            //     validationError: errorMessage | null
            // }
            // obtained either from the Payload validation process or as a result of validating a
            // terminal PoW block referenced by forkchoiceState.headBlockHash
            return Ok(ForkchoiceUpdated::from_status(PayloadStatusEnum::Invalid {
                validation_error: EngineApiError::PayloadPreMerge.to_string(),
            })
            .with_latest_valid_hash(H256::zero()))
        }

        if let Some(_attr) = payload_attributes {
            // TODO: optionally build the block
        }

        let chain_info = self.client.chain_info()?;
        Ok(ForkchoiceUpdated::from_status(PayloadStatusEnum::Valid)
            .with_latest_valid_hash(chain_info.best_hash))
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

        // Short circuit if communicated block hash is zero
        if terminal_block_hash.is_zero() {
            return Ok(TransitionConfiguration {
                terminal_total_difficulty: merge_terminal_td,
                ..Default::default()
            })
        }

        // Attempt to look up terminal block hash
        let local_hash = self.client.block_hash(terminal_block_number.as_u64())?;

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
}

impl<Client> Future for EngineApi<Client>
where
    Client: HeaderProvider + BlockProvider + StateProviderFactory + EvmEnvProvider + Unpin,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        loop {
            match ready!(this.message_rx.poll_next_unpin(cx)) {
                Some(msg) => this.on_message(msg),
                None => {
                    // channel closed
                    return Poll::Ready(())
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use reth_interfaces::test_utils::generators::random_block;
    use reth_primitives::{H256, MAINNET};
    use reth_provider::test_utils::MockEthProvider;
    use std::sync::Arc;
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

    fn setup_engine_api() -> (EngineApiTestHandle, EngineApi<Arc<MockEthProvider>>) {
        let chain_spec = Arc::new(MAINNET.clone());
        let client = Arc::new(MockEthProvider::default());
        let (msg_tx, msg_rx) = unbounded_channel();
        let (engine_tx, engine_rx) = mpsc::unbounded_channel();
        let api = EngineApi {
            client: client.clone(),
            chain_spec: chain_spec.clone(),
            message_rx: UnboundedReceiverStream::new(msg_rx),
            engine_tx,
        };
        let handle = EngineApiTestHandle { chain_spec, client, msg_tx, engine_rx };
        (handle, api)
    }

    struct EngineApiTestHandle {
        chain_spec: Arc<ChainSpec>,
        client: Arc<MockEthProvider>,
        msg_tx: UnboundedSender<EngineApiMessage>,
        engine_rx: UnboundedReceiver<BeaconEngineMessage>,
    }

    impl EngineApiTestHandle {
        fn send_message(&self, msg: EngineApiMessage) {
            self.msg_tx.send(msg).expect("failed to send engine msg");
        }
    }

    // mod new_payload {
    //     use super::*;
    //     use reth_primitives::{proofs, Block, Header};

    //     // TODO: remove?
    //     fn transform_block<F: FnOnce(Block) -> Block>(src: SealedBlock, f: F) -> SealedBlock {
    //         let unsealed = src.unseal();
    //         let mut transformed: Block = f(unsealed);
    //         // Recalculate roots
    //         transformed.header.transactions_root =
    //             proofs::calculate_transaction_root(transformed.body.iter());
    //         transformed.header.ommers_hash =
    //             proofs::calculate_ommers_root(transformed.ommers.iter());
    //         SealedBlock {
    //             header: transformed.header.seal_slow(),
    //             body: transformed.body,
    //             ommers: transformed.ommers.into_iter().map(Header::seal_slow).collect(),
    //             withdrawals: transformed.withdrawals,
    //         }
    //     }

    //     #[tokio::test]
    //     async fn payload_known() {
    //         let (handle, api) = setup_engine_api();
    //         tokio::spawn(api);

    //         let block = random_block(100, Some(H256::random()), None, Some(0)); // payload must
    // have no ommers         let block_hash = block.hash();
    //         let execution_payload = block.clone().into();

    //         handle.client.add_header(block_hash, block.header.unseal());

    //         let (result_tx, result_rx) = oneshot::channel();
    //         handle.send_message(EngineApiMessage::NewPayload(
    //             EngineApiMessageVersion::V1,
    //             execution_payload,
    //             result_tx,
    //         ));

    //         let expected_result = PayloadStatus::from_status(PayloadStatusEnum::Valid)
    //             .with_latest_valid_hash(block_hash);
    //         assert_matches!(result_rx.await, Ok(Ok(result)) => assert_eq!(result,
    // expected_result));     }

    //     #[tokio::test]
    //     async fn payload_parent_unknown() {
    //         let (handle, api) = setup_engine_api();
    //         tokio::spawn(api);

    //         let (result_tx, result_rx) = oneshot::channel();
    //         let block = random_block(100, Some(H256::random()), None, Some(0)); // payload must
    // have no ommers         handle.send_message(EngineApiMessage::NewPayload(
    //             EngineApiMessageVersion::V1,
    //             block.into(),
    //             result_tx,
    //         ));

    //         let expected_result = PayloadStatus::from_status(PayloadStatusEnum::Syncing);
    //         assert_matches!(result_rx.await, Ok(Ok(result)) => assert_eq!(result,
    // expected_result));     }

    //     #[tokio::test]
    //     async fn payload_pre_merge() {
    //         let (handle, api) = setup_engine_api();
    //         tokio::spawn(api);

    //         let parent = transform_block(random_block(100, None, None, Some(0)), |mut b| {
    //             b.header.difficulty =
    //                 handle.chain_spec.fork(Hardfork::Paris).ttd().unwrap() - U256::from(1);
    //             b
    //         });
    //         let block = random_block(101, Some(parent.hash()), None, Some(0));

    //         handle.client.add_block(parent.hash(), parent.clone().unseal());

    //         let (result_tx, result_rx) = oneshot::channel();
    //         handle.send_message(EngineApiMessage::NewPayload(
    //             EngineApiMessageVersion::V1,
    //             block.clone().into(),
    //             result_tx,
    //         ));

    //         let expected_result = PayloadStatus::from_status(PayloadStatusEnum::Invalid {
    //             validation_error: EngineApiError::PayloadPreMerge.to_string(),
    //         })
    //         .with_latest_valid_hash(H256::zero());
    //         assert_matches!(result_rx.await, Ok(Ok(result)) => assert_eq!(result,
    // expected_result));     }

    //     #[tokio::test]
    //     async fn invalid_payload_timestamp() {
    //         let (handle, api) = setup_engine_api();
    //         tokio::spawn(api);

    //         let block_timestamp = 100;
    //         let parent_timestamp = block_timestamp + 10;
    //         let parent = transform_block(random_block(100, None, None, Some(0)), |mut b| {
    //             b.header.timestamp = parent_timestamp;
    //             b.header.difficulty =
    //                 handle.chain_spec.fork(Hardfork::Paris).ttd().unwrap() + U256::from(1);
    //             b
    //         });
    //         let block =
    //             transform_block(random_block(101, Some(parent.hash()), None, Some(0)), |mut b| {
    //                 b.header.timestamp = block_timestamp;
    //                 b
    //             });

    //         handle.client.add_block(parent.hash(), parent.clone().unseal());

    //         let (result_tx, result_rx) = oneshot::channel();
    //         handle.send_message(EngineApiMessage::NewPayload(
    //             EngineApiMessageVersion::V1,
    //             block.clone().into(),
    //             result_tx,
    //         ));

    //         let expected_result = PayloadStatus::from_status(PayloadStatusEnum::Invalid {
    //             validation_error: EngineApiError::PayloadTimestamp {
    //                 invalid: block_timestamp,
    //                 latest: parent_timestamp,
    //             }
    //             .to_string(),
    //         });
    //         assert_matches!( result_rx.await, Ok(Ok(result)) => assert_eq!(result,
    // expected_result));     }
    // }

    // tests covering `engine_getPayloadBodiesByRange` and `engine_getPayloadBodiesByHash`
    mod get_payload_bodies {
        use super::*;
        use reth_interfaces::test_utils::generators::random_block_range;

        #[tokio::test]
        async fn invalid_params() {
            let (handle, api) = setup_engine_api();
            tokio::spawn(api);

            let by_range_tests = [
                // (start, count)
                (0, 0),
                (0, 1),
                (1, 0),
            ];

            // test [EngineApiMessage::GetPayloadBodiesByRange]
            for (start, count) in by_range_tests {
                let (result_tx, result_rx) = oneshot::channel();
                handle.send_message(EngineApiMessage::GetPayloadBodiesByRange(
                    start, count, result_tx,
                ));
                assert_matches!(result_rx.await, Ok(Err(EngineApiError::InvalidParams)));
            }
        }

        #[tokio::test]
        async fn request_too_large() {
            let (handle, api) = setup_engine_api();
            tokio::spawn(api);

            let request_count = MAX_PAYLOAD_BODIES_LIMIT + 1;

            let (result_tx, result_rx) = oneshot::channel();
            handle.send_message(EngineApiMessage::GetPayloadBodiesByRange(
                0,
                request_count,
                result_tx,
            ));
            assert_matches!(
                result_rx.await,
                Ok(Err(EngineApiError::PayloadRequestTooLarge { .. }))
            );

            let (result_tx, result_rx) = oneshot::channel();
            let hashes = std::iter::repeat(H256::default()).take(request_count as usize).collect();
            handle.send_message(EngineApiMessage::GetPayloadBodiesByHash(hashes, result_tx));
            assert_matches!(result_rx.await, Ok(Err(EngineApiError::PayloadRequestTooLarge { .. })))
        }

        #[tokio::test]
        async fn returns_payload_bodies() {
            let (handle, api) = setup_engine_api();
            tokio::spawn(api);

            let (start, count) = (1, 10);
            let blocks = random_block_range(start..start + count, H256::default(), 0..2);
            handle.client.extend_blocks(blocks.iter().cloned().map(|b| (b.hash(), b.unseal())));

            let expected =
                blocks.iter().cloned().map(|b| Some(b.unseal().into())).collect::<Vec<_>>();

            let (result_tx, result_rx) = oneshot::channel();
            handle.send_message(EngineApiMessage::GetPayloadBodiesByRange(start, count, result_tx));
            assert_matches!(result_rx.await, Ok(Ok(result)) => assert_eq!(result, expected));

            let (result_tx, result_rx) = oneshot::channel();
            let hashes = blocks.iter().map(|b| b.hash()).collect();
            handle.send_message(EngineApiMessage::GetPayloadBodiesByHash(hashes, result_tx));
            assert_matches!(result_rx.await, Ok(Ok(result)) => assert_eq!(result, expected));
        }

        #[tokio::test]
        async fn returns_payload_bodies_with_gaps() {
            let (handle, api) = setup_engine_api();
            tokio::spawn(api);

            let (start, count) = (1, 100);
            let blocks = random_block_range(start..start + count, H256::default(), 0..2);

            // Insert only blocks in ranges 1-25 and 50-75
            let first_missing_range = 26..=50;
            let second_missing_range = 76..=100;
            handle.client.extend_blocks(
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

            let (result_tx, result_rx) = oneshot::channel();
            handle.send_message(EngineApiMessage::GetPayloadBodiesByRange(start, count, result_tx));
            assert_matches!(result_rx.await, Ok(Ok(result)) => assert_eq!(result, expected));

            let (result_tx, result_rx) = oneshot::channel();
            let hashes = blocks.iter().map(|b| b.hash()).collect();
            handle.send_message(EngineApiMessage::GetPayloadBodiesByHash(hashes, result_tx));
            assert_matches!(result_rx.await, Ok(Ok(result)) => assert_eq!(result, expected));
        }
    }

    // mod fork_choice_updated {
    //     use super::*;
    //     use reth_interfaces::test_utils::generators::random_header;

    //     #[tokio::test]
    //     async fn empty_head() {
    //         let (handle, api) = setup_engine_api();
    //         tokio::spawn(api);

    //         let (result_tx, result_rx) = oneshot::channel();
    //         handle.send_message(EngineApiMessage::ForkchoiceUpdated(
    //             EngineApiMessageVersion::V1,
    //             ForkchoiceState::default(),
    //             None,
    //             result_tx,
    //         ));

    //         let expected_result = ForkchoiceUpdated::from_status(PayloadStatusEnum::Invalid {
    //             validation_error: EngineApiError::ForkchoiceEmptyHead.to_string(),
    //         });
    //         assert_matches!(result_rx.await, Ok(Ok(result)) => assert_eq!(result,
    // expected_result));     }

    //     #[tokio::test]
    //     async fn unknown_head_hash() {
    //         let (handle, api) = setup_engine_api();
    //         tokio::spawn(api);

    //         let state = ForkchoiceState { head_block_hash: H256::random(), ..Default::default()
    // };

    //         let (result_tx, result_rx) = oneshot::channel();
    //         handle.send_message(EngineApiMessage::ForkchoiceUpdated(
    //             EngineApiMessageVersion::V1,
    //             state,
    //             None,
    //             result_tx,
    //         ));

    //         let expected_result = ForkchoiceUpdated::from_status(PayloadStatusEnum::Syncing);
    //         assert_matches!(result_rx.await, Ok(Ok(result)) => assert_eq!(result,
    // expected_result));     }

    //     #[tokio::test]
    //     async fn unknown_finalized_hash() {
    //         let (handle, api) = setup_engine_api();
    //         tokio::spawn(api);

    //         let head = random_header(100, None);
    //         handle.client.add_header(head.hash(), head.clone().unseal());

    //         let state = ForkchoiceState {
    //             head_block_hash: head.hash(),
    //             finalized_block_hash: H256::random(),
    //             ..Default::default()
    //         };

    //         let (result_tx, result_rx) = oneshot::channel();
    //         handle.send_message(EngineApiMessage::ForkchoiceUpdated(
    //             EngineApiMessageVersion::V1,
    //             state,
    //             None,
    //             result_tx,
    //         ));

    //         let expected_result = ForkchoiceUpdated::from_status(PayloadStatusEnum::Syncing);
    //         assert_matches!(result_rx.await, Ok(Ok(result)) => assert_eq!(result,
    // expected_result));     }

    //     #[tokio::test]
    //     async fn forkchoice_state_is_updated() {
    //         let (handle, api) = setup_engine_api();
    //         tokio::spawn(api);

    //         let ttd = handle.chain_spec.fork(Hardfork::Paris).ttd().unwrap();
    //         let finalized = random_header(90, None);
    //         let mut head = random_header(100, None).unseal();

    //         // set the difficulty so we know it is post-merge
    //         head.difficulty = ttd;
    //         let head = head.seal_slow();
    //         handle.client.extend_headers([
    //             (head.hash(), head.clone().unseal()),
    //             (finalized.hash(), finalized.clone().unseal()),
    //         ]);

    //         let state = ForkchoiceState {
    //             head_block_hash: head.hash(),
    //             finalized_block_hash: finalized.hash(),
    //             ..Default::default()
    //         };

    //         let (result_tx, result_rx) = oneshot::channel();
    //         handle.send_message(EngineApiMessage::ForkchoiceUpdated(
    //             EngineApiMessageVersion::V1,
    //             state.clone(),
    //             None,
    //             result_tx,
    //         ));

    //         let expected_result = ForkchoiceUpdated {
    //             payload_id: None,
    //             payload_status: PayloadStatus {
    //                 status: PayloadStatusEnum::Valid,
    //                 latest_valid_hash: Some(head.hash()),
    //             },
    //         };
    //         assert_matches!(result_rx.await, Ok(Ok(result)) => assert_eq!(result,
    // expected_result));     }

    //     #[tokio::test]
    //     async fn forkchoice_updated_invalid_pow() {
    //         let (handle, api) = setup_engine_api();
    //         tokio::spawn(api);

    //         let finalized = random_header(90, None);
    //         let mut head = random_header(100, None).unseal();

    //         // ensure we don't mess up when subtracting just in case
    //         let ttd = handle.chain_spec.fork(Hardfork::Paris).ttd().unwrap();
    //         assert!(ttd > finalized.difficulty);

    //         // set the difficulty so we know it is post-merge
    //         head.difficulty = ttd - U256::from(1) - finalized.difficulty;
    //         let head = head.seal_slow();
    //         handle.client.extend_headers([
    //             (head.hash(), head.clone().unseal()),
    //             (finalized.hash(), finalized.clone().unseal()),
    //         ]);

    //         let state = ForkchoiceState {
    //             head_block_hash: head.hash(),
    //             finalized_block_hash: finalized.hash(),
    //             ..Default::default()
    //         };

    //         let (result_tx, result_rx) = oneshot::channel();
    //         handle.send_message(EngineApiMessage::ForkchoiceUpdated(
    //             EngineApiMessageVersion::V1,
    //             state.clone(),
    //             None,
    //             result_tx,
    //         ));

    //         let expected_result = ForkchoiceUpdated {
    //             payload_id: None,
    //             payload_status: PayloadStatus {
    //                 status: PayloadStatusEnum::Invalid {
    //                     validation_error: EngineApiError::PayloadPreMerge.to_string(),
    //                 },
    //                 latest_valid_hash: Some(H256::zero()),
    //             },
    //         };
    //         assert_matches!(result_rx.await, Ok(Ok(result)) => assert_eq!(result,
    // expected_result));     }
    // }

    // https://github.com/ethereum/execution-apis/blob/main/src/engine/paris.md#specification-3
    mod exchange_transition_configuration {
        use super::*;

        #[tokio::test]
        async fn terminal_td_mismatch() {
            let (handle, api) = setup_engine_api();
            tokio::spawn(api);

            let transition_config = TransitionConfiguration {
                terminal_total_difficulty: handle.chain_spec.fork(Hardfork::Paris).ttd().unwrap() +
                    U256::from(1),
                ..Default::default()
            };

            let (result_tx, result_rx) = oneshot::channel();
            handle.send_message(EngineApiMessage::ExchangeTransitionConfiguration(
                transition_config.clone(),
                result_tx,
            ));

            assert_matches!(
                result_rx.await,
                Ok(Err(EngineApiError::TerminalTD { execution, consensus }))
                    if execution == handle.chain_spec.fork(Hardfork::Paris).ttd().unwrap() && consensus == U256::from(transition_config.terminal_total_difficulty)
            );
        }

        #[tokio::test]
        async fn terminal_block_hash_mismatch() {
            let (handle, api) = setup_engine_api();
            tokio::spawn(api);

            let terminal_block_number = 1000;
            let consensus_terminal_block = random_block(terminal_block_number, None, None, None);
            let execution_terminal_block = random_block(terminal_block_number, None, None, None);

            let transition_config = TransitionConfiguration {
                terminal_total_difficulty: handle.chain_spec.fork(Hardfork::Paris).ttd().unwrap(),
                terminal_block_hash: consensus_terminal_block.hash(),
                terminal_block_number: terminal_block_number.into(),
            };

            // Unknown block number
            let (result_tx, result_rx) = oneshot::channel();
            handle.send_message(EngineApiMessage::ExchangeTransitionConfiguration(
                transition_config.clone(),
                result_tx,
            ));
            assert_matches!(
                result_rx.await,
                Ok(Err(EngineApiError::TerminalBlockHash { execution, consensus }))
                    if execution == None && consensus == transition_config.terminal_block_hash
            );

            // Add block and to provider local store and test for mismatch
            handle.client.add_block(
                execution_terminal_block.hash(),
                execution_terminal_block.clone().unseal(),
            );

            let (result_tx, result_rx) = oneshot::channel();
            handle.send_message(EngineApiMessage::ExchangeTransitionConfiguration(
                transition_config.clone(),
                result_tx,
            ));

            assert_matches!(
                result_rx.await,
                Ok(Err(EngineApiError::TerminalBlockHash { execution, consensus }))
                    if execution == Some(execution_terminal_block.hash()) && consensus == transition_config.terminal_block_hash
            );
        }

        #[tokio::test]
        async fn configurations_match() {
            let (handle, api) = setup_engine_api();
            tokio::spawn(api);

            let terminal_block_number = 1000;
            let terminal_block = random_block(terminal_block_number, None, None, None);

            let transition_config = TransitionConfiguration {
                terminal_total_difficulty: handle.chain_spec.fork(Hardfork::Paris).ttd().unwrap(),
                terminal_block_hash: terminal_block.hash(),
                terminal_block_number: terminal_block_number.into(),
            };

            handle.client.add_block(terminal_block.hash(), terminal_block.clone().unseal());

            let (result_tx, result_rx) = oneshot::channel();
            handle.send_message(EngineApiMessage::ExchangeTransitionConfiguration(
                transition_config.clone(),
                result_tx,
            ));

            assert_matches!(result_rx.await, Ok(Ok(config)) => assert_eq!(config, transition_config));
        }
    }
}

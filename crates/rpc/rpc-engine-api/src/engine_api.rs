use crate::{EngineApiError, EngineApiMessage, EngineApiResult};
use futures::StreamExt;
use reth_beacon_consensus::BeaconEngineMessage;
use reth_primitives::{BlockHash, BlockId, BlockNumber, ChainSpec, Hardfork};
use reth_provider::{BlockProvider, EvmEnvProvider, HeaderProvider, StateProviderFactory};
use reth_rpc_types::engine::{ExecutionPayloadBodies, TransitionConfiguration};
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
                // forward message to the consensus engine
                let _ = self.engine_tx.send(BeaconEngineMessage::GetPayload { payload_id, tx });
            }
            EngineApiMessage::GetPayloadBodiesByHash(hashes, tx) => {
                let _ = tx.send(self.get_payload_bodies_by_hash(hashes));
            }
            EngineApiMessage::GetPayloadBodiesByRange(start, count, tx) => {
                let _ = tx.send(self.get_payload_bodies_by_range(start, count));
            }
            EngineApiMessage::NewPayload(payload, tx) => {
                // forward message to the consensus engine
                let _ = self.engine_tx.send(BeaconEngineMessage::NewPayload { payload, tx });
            }
            EngineApiMessage::ForkchoiceUpdated(state, payload_attrs, tx) => {
                // forward message to the consensus engine
                let _ = self.engine_tx.send(BeaconEngineMessage::ForkchoiceUpdated {
                    state,
                    payload_attrs,
                    tx,
                });
            }
        }
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
            return Err(EngineApiError::InvalidBodiesRange { start, count })
        }

        let mut result = Vec::with_capacity(count as usize);
        for num in start..start + count {
            let block = self
                .client
                .block(BlockId::Number(num.into()))
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
                .client
                .block(BlockId::Hash(hash.into()))
                .map_err(|err| EngineApiError::Internal(Box::new(err)))?;
            result.push(block.map(Into::into));
        }

        Ok(result)
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
        let local_hash = self
            .client
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
    use reth_interfaces::{consensus::ForkchoiceState, test_utils::generators::random_block};
    use reth_primitives::{SealedBlock, H256, MAINNET};
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

    #[tokio::test]
    async fn forwards_responses_to_consensus_engine() {
        let (mut handle, api) = setup_engine_api();
        tokio::spawn(api);

        let (result_tx, _result_rx) = oneshot::channel();
        handle.send_message(EngineApiMessage::NewPayload(SealedBlock::default().into(), result_tx));
        assert_matches!(
            handle.engine_rx.recv().await,
            Some(BeaconEngineMessage::NewPayload { .. })
        );

        let (result_tx, _result_rx) = oneshot::channel();
        handle.send_message(EngineApiMessage::ForkchoiceUpdated(
            ForkchoiceState::default(),
            None,
            result_tx,
        ));
        assert_matches!(
            handle.engine_rx.recv().await,
            Some(BeaconEngineMessage::ForkchoiceUpdated { .. })
        );
    }

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
                assert_matches!(
                    result_rx.await,
                    Ok(Err(EngineApiError::InvalidBodiesRange { .. }))
                );
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

    // https://github.com/ethereum/execution-apis/blob/main/src/engine/paris.md#specification-3
    mod exchange_transition_configuration {
        use super::*;
        use reth_primitives::U256;

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
                    if execution.is_none() && consensus == transition_config.terminal_block_hash
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

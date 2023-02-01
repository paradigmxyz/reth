use crate::{EngineApiError, EngineApiMessage, EngineApiResult};
use futures::StreamExt;
use reth_executor::{
    executor,
    revm_wrap::{State, SubState},
};
use reth_interfaces::consensus::ForkchoiceState;
use reth_primitives::{
    proofs::{self, EMPTY_LIST_HASH},
    rpc::{BlockId, H256 as EthersH256},
    ChainSpec, Header, SealedBlock, TransactionSigned, H64, U256,
};
use reth_provider::{BlockProvider, HeaderProvider, StateProvider};
use reth_rlp::Decodable;
use reth_rpc_types::engine::{
    ExecutionPayload, ForkchoiceUpdated, PayloadAttributes, PayloadStatus, PayloadStatusEnum,
    TransitionConfiguration,
};
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::sync::oneshot;
use tokio_stream::wrappers::UnboundedReceiverStream;

/// The Engine API response sender
pub type EngineApiSender<Ok> = oneshot::Sender<EngineApiResult<Ok>>;

/// The Engine API implementation that grants the Consensus layer access to data and
/// functions in the Execution layer that are crucial for the consensus process.
#[must_use = "EngineApi does nothing unless polled."]
pub struct EngineApi<Client> {
    client: Arc<Client>,
    /// Consensus configuration
    chain_spec: ChainSpec,
    rx: UnboundedReceiverStream<EngineApiMessage>,
    // TODO: Placeholder for storing future blocks. Make cache bounded.
    // Use [lru](https://crates.io/crates/lru) crate
    local_store: HashMap<H64, ExecutionPayload>,
    // remote_store: HashMap<H64, ExecutionPayload>,
}

impl<Client: HeaderProvider + BlockProvider + StateProvider> EngineApi<Client> {
    fn on_message(&mut self, msg: EngineApiMessage) {
        match msg {
            EngineApiMessage::GetPayload(payload_id, tx) => {
                // NOTE: Will always result in `PayloadUnknown` since we don't support block
                // building for now.
                let _ = tx.send(self.get_payload(payload_id).ok_or(EngineApiError::PayloadUnknown));
            }
            EngineApiMessage::NewPayload(payload, tx) => {
                let _ = tx.send(self.new_payload(payload));
            }
            EngineApiMessage::ForkchoiceUpdated(state, attrs, tx) => {
                let _ = tx.send(self.fork_choice_updated(state, attrs));
            }
            EngineApiMessage::ExchangeTransitionConfiguration(config, tx) => {
                let _ = tx.send(self.exchange_transition_configuration(config));
            }
        }
    }

    /// Try to construct a block from given payload. Perform addition validation of `extra_data` and
    /// `base_fee_per_gas` fields.
    ///
    /// NOTE: The log bloom is assumed to be validated during serialization.
    /// NOTE: Empty ommers, nonce and difficulty values are validated upon computing block hash and
    /// comparing the value with `payload.block_hash`.
    ///
    /// See <https://github.com/ethereum/go-ethereum/blob/79a478bb6176425c2400e949890e668a3d9a3d05/core/beacon/types.go#L145>
    fn try_construct_block(&self, payload: ExecutionPayload) -> EngineApiResult<SealedBlock> {
        if payload.extra_data.len() > 32 {
            return Err(EngineApiError::PayloadExtraData(payload.extra_data))
        }

        if payload.base_fee_per_gas == U256::ZERO {
            return Err(EngineApiError::PayloadBaseFee(payload.base_fee_per_gas))
        }

        let transactions = payload
            .transactions
            .iter()
            .map(|tx| TransactionSigned::decode(&mut tx.as_ref()))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let transactions_root = proofs::calculate_transaction_root(transactions.iter());
        let header = Header {
            parent_hash: payload.parent_hash,
            beneficiary: payload.fee_recipient,
            state_root: payload.state_root,
            transactions_root,
            receipts_root: payload.receipts_root,
            logs_bloom: payload.logs_bloom,
            number: payload.block_number.as_u64(),
            gas_limit: payload.gas_limit.as_u64(),
            gas_used: payload.gas_used.as_u64(),
            timestamp: payload.timestamp.as_u64(),
            mix_hash: payload.prev_randao,
            base_fee_per_gas: Some(payload.base_fee_per_gas.to::<u64>()),
            extra_data: payload.extra_data,
            // Defaults
            ommers_hash: EMPTY_LIST_HASH,
            difficulty: Default::default(),
            nonce: Default::default(),
        };
        let header = header.seal();

        if payload.block_hash != header.hash() {
            return Err(EngineApiError::PayloadBlockHash {
                execution: header.hash(),
                consensus: payload.block_hash,
            })
        }

        Ok(SealedBlock { header, body: transactions, ommers: Default::default() })
    }

    /// Called to retrieve the latest state of the network, validate new blocks, and maintain
    /// consistency between the Consensus and Execution layers.
    pub fn get_payload(&self, payload_id: H64) -> Option<ExecutionPayload> {
        self.local_store.get(&payload_id).cloned()
    }

    /// When the Consensus layer receives a new block via the consensus gossip protocol,
    /// the transactions in the block are sent to the execution layer in the form of a
    /// `ExecutionPayload`. The Execution layer executes the transactions and validates the
    /// state in the block header, then passes validation data back to Consensus layer, that
    /// adds the block to the head of its own blockchain and attests to it. The block is then
    /// broadcasted over the consensus p2p network in the form of a "Beacon block".
    pub fn new_payload(&mut self, payload: ExecutionPayload) -> EngineApiResult<PayloadStatus> {
        let block = match self.try_construct_block(payload) {
            Ok(b) => b,
            Err(err) => {
                return Ok(PayloadStatus::from_status(PayloadStatusEnum::InvalidBlockHash {
                    validation_error: err.to_string(),
                }))
            }
        };
        let block_hash = block.header.hash();
        let parent_hash = block.parent_hash;

        // The block already exists in our database
        if self.client.is_known(&block_hash)? {
            return Ok(PayloadStatus::new(PayloadStatusEnum::Valid, block_hash))
        }

        let Some(parent) = self.client.block(BlockId::Hash(EthersH256(parent_hash.0)))? else {
             // TODO: cache block for storing later
             return Ok(PayloadStatus::from_status(PayloadStatusEnum::Syncing))
        };

        if let Some(parent_td) = self.client.header_td(&parent_hash)? {
            if Some(parent_td) <= self.chain_spec.paris_status().terminal_total_difficulty() {
                return Ok(PayloadStatus::from_status(PayloadStatusEnum::Invalid {
                    validation_error: EngineApiError::PayloadPreMerge.to_string(),
                }))
            }
        }

        if block.timestamp <= parent.timestamp {
            return Ok(PayloadStatus::from_status(PayloadStatusEnum::Invalid {
                validation_error: EngineApiError::PayloadTimestamp {
                    invalid: block.timestamp,
                    latest: parent.timestamp,
                }
                .to_string(),
            }))
        }

        let mut state_provider = SubState::new(State::new(&*self.client));
        match executor::execute_and_verify_receipt(
            &block.unseal(),
            None,
            &self.chain_spec,
            &mut state_provider,
        ) {
            Ok(_) => Ok(PayloadStatus::new(PayloadStatusEnum::Valid, block_hash)),
            Err(err) => Ok(PayloadStatus::new(
                PayloadStatusEnum::Invalid { validation_error: err.to_string() },
                parent_hash, // The parent hash is already in our database hence it is valid
            )),
        }
    }

    /// Called to resolve chain forks and ensure that the Execution layer is working with the latest
    /// valid chain.
    pub fn fork_choice_updated(
        &self,
        fork_choice_state: ForkchoiceState,
        _payload_attributes: Option<PayloadAttributes>,
    ) -> EngineApiResult<ForkchoiceUpdated> {
        let ForkchoiceState { head_block_hash, finalized_block_hash, .. } = fork_choice_state;

        if head_block_hash.is_zero() {
            return Ok(ForkchoiceUpdated::from_status(PayloadStatusEnum::Invalid {
                validation_error: EngineApiError::ForkchoiceEmptyHead.to_string(),
            }))
        }

        // Block is not known, nothing to do.
        if !self.client.is_known(&head_block_hash)? {
            return Ok(ForkchoiceUpdated::from_status(PayloadStatusEnum::Syncing))
        }

        // The finalized block hash is not known, we are still syncing
        if !finalized_block_hash.is_zero() && !self.client.is_known(&finalized_block_hash)? {
            return Ok(ForkchoiceUpdated::from_status(PayloadStatusEnum::Syncing))
        }

        // TODO: update tip

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
            .paris_status()
            .terminal_total_difficulty()
            .ok_or(EngineApiError::UnknownMergeTerminalTotalDifficulty)?;

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
        let local_hash = self.client.block_hash(U256::from(terminal_block_number.as_u64()))?;

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
    Client: HeaderProvider + BlockProvider + StateProvider + Unpin,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        loop {
            match ready!(this.rx.poll_next_unpin(cx)) {
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
    use reth_primitives::H256;
    use reth_provider::test_utils::MockEthProvider;
    use tokio::sync::mpsc::unbounded_channel;

    mod new_payload {
        use super::*;
        use bytes::{Bytes, BytesMut};
        use reth_interfaces::test_utils::generators::random_header;
        use reth_primitives::{Block, MAINNET};
        use reth_rlp::DecodeError;

        fn transform_block<F: FnOnce(Block) -> Block>(src: SealedBlock, f: F) -> SealedBlock {
            let unsealed = src.unseal();
            let mut transformed: Block = f(unsealed);
            // Recalculate roots
            transformed.header.transactions_root =
                proofs::calculate_transaction_root(transformed.body.iter());
            transformed.header.ommers_hash =
                proofs::calculate_ommers_root(transformed.ommers.iter());
            SealedBlock {
                header: transformed.header.seal(),
                body: transformed.body,
                ommers: transformed.ommers.into_iter().map(Header::seal).collect(),
            }
        }

        #[tokio::test]
        async fn payload_validation() {
            let (_tx, rx) = unbounded_channel();
            let engine = EngineApi {
                client: Arc::new(MockEthProvider::default()),
                chain_spec: MAINNET.clone(),
                local_store: Default::default(),
                rx: UnboundedReceiverStream::new(rx),
            };

            let block = random_block(100, Some(H256::random()), Some(3), Some(0));

            // Valid extra data
            let block_with_valid_extra_data = transform_block(block.clone(), |mut b| {
                b.header.extra_data = BytesMut::zeroed(32).freeze().into();
                b
            });
            assert_matches!(engine.try_construct_block(block_with_valid_extra_data.into()), Ok(_));

            // Invalid extra data
            let block_with_invalid_extra_data: Bytes = BytesMut::zeroed(33).freeze();
            let invalid_extra_data_block = transform_block(block.clone(), |mut b| {
                b.header.extra_data = block_with_invalid_extra_data.clone().into();
                b
            });
            assert_matches!(
                engine.try_construct_block(invalid_extra_data_block.into()),
                Err(EngineApiError::PayloadExtraData(data)) if data == block_with_invalid_extra_data
            );

            // Zero base fee
            let block_with_zero_base_fee = transform_block(block.clone(), |mut b| {
                b.header.base_fee_per_gas = Some(0);
                b
            });
            assert_matches!(
                engine.try_construct_block(block_with_zero_base_fee.into()),
                Err(EngineApiError::PayloadBaseFee(val)) if val == U256::ZERO
            );

            // Invalid encoded transactions
            let mut payload_with_invalid_txs: ExecutionPayload = block.clone().into();
            payload_with_invalid_txs.transactions.iter_mut().for_each(|tx| {
                *tx = Bytes::new().into();
            });
            assert_matches!(
                engine.try_construct_block(payload_with_invalid_txs),
                Err(EngineApiError::Decode(DecodeError::InputTooShort))
            );

            // Non empty ommers
            let block_with_ommers = transform_block(block.clone(), |mut b| {
                b.ommers.push(random_header(100, None).unseal());
                b
            });
            assert_matches!(
                engine.try_construct_block(block_with_ommers.clone().into()),
                Err(EngineApiError::PayloadBlockHash { consensus, .. })
                    if consensus == block_with_ommers.hash()
            );

            // None zero difficulty
            let block_with_difficulty = transform_block(block.clone(), |mut b| {
                b.header.difficulty = U256::from(1);
                b
            });
            assert_matches!(
                engine.try_construct_block(block_with_difficulty.clone().into()),
                Err(EngineApiError::PayloadBlockHash { consensus, .. })
                    if consensus == block_with_difficulty.hash()
            );

            // None zero nonce
            let block_with_nonce = transform_block(block.clone(), |mut b| {
                b.header.nonce = 1;
                b
            });
            assert_matches!(
                engine.try_construct_block(block_with_nonce.clone().into()),
                Err(EngineApiError::PayloadBlockHash { consensus, .. })
                    if consensus == block_with_nonce.hash()
            );

            // Valid block
            let valid_block = block;
            assert_matches!(engine.try_construct_block(valid_block.into()), Ok(_));
        }

        #[tokio::test]
        async fn payload_known() {
            let (tx, rx) = unbounded_channel();
            let client = Arc::new(MockEthProvider::default());
            let engine = EngineApi {
                client: client.clone(),
                chain_spec: MAINNET.clone(),
                local_store: Default::default(),
                rx: UnboundedReceiverStream::new(rx),
            };

            tokio::spawn(engine);

            let block = random_block(100, Some(H256::random()), None, Some(0)); // payload must have no ommers
            let block_hash = block.hash();
            let execution_payload = block.clone().into();

            client.add_header(block_hash, block.header.unseal());

            let (result_tx, result_rx) = oneshot::channel();
            tx.send(EngineApiMessage::NewPayload(execution_payload, result_tx))
                .expect("failed to send engine msg");

            let result = result_rx.await;
            assert_matches!(result, Ok(Ok(_)));
            let expected_result = PayloadStatus::new(PayloadStatusEnum::Valid, block_hash);
            assert_eq!(result.unwrap().unwrap(), expected_result);
        }

        #[tokio::test]
        async fn payload_parent_unknown() {
            let (tx, rx) = unbounded_channel();
            let engine = EngineApi {
                client: Arc::new(MockEthProvider::default()),
                chain_spec: MAINNET.clone(),
                local_store: Default::default(),
                rx: UnboundedReceiverStream::new(rx),
            };

            tokio::spawn(engine);

            let (result_tx, result_rx) = oneshot::channel();
            let block = random_block(100, Some(H256::random()), None, Some(0)); // payload must have no ommers
            tx.send(EngineApiMessage::NewPayload(block.into(), result_tx))
                .expect("failed to send engine msg");

            let result = result_rx.await;
            assert_matches!(result, Ok(Ok(_)));
            let expected_result = PayloadStatus::from_status(PayloadStatusEnum::Syncing);
            assert_eq!(result.unwrap().unwrap(), expected_result);
        }

        #[tokio::test]
        async fn payload_pre_merge() {
            let (tx, rx) = unbounded_channel();
            let chain_spec = MAINNET.clone();
            let client = Arc::new(MockEthProvider::default());
            let engine = EngineApi {
                client: client.clone(),
                chain_spec: chain_spec.clone(),
                local_store: Default::default(),
                rx: UnboundedReceiverStream::new(rx),
            };

            tokio::spawn(engine);

            let (result_tx, result_rx) = oneshot::channel();
            let parent = transform_block(random_block(100, None, None, Some(0)), |mut b| {
                b.header.difficulty =
                    chain_spec.paris_status().terminal_total_difficulty().unwrap();
                b
            });
            let block = random_block(101, Some(parent.hash()), None, Some(0));

            client.add_block(parent.hash(), parent.clone().unseal());

            tx.send(EngineApiMessage::NewPayload(block.clone().into(), result_tx))
                .expect("failed to send engine msg");

            let result = result_rx.await;
            assert_matches!(result, Ok(Ok(_)));
            let expected_result = PayloadStatus::from_status(PayloadStatusEnum::Invalid {
                validation_error: EngineApiError::PayloadPreMerge.to_string(),
            });
            assert_eq!(result.unwrap().unwrap(), expected_result);
        }

        #[tokio::test]
        async fn invalid_payload_timestamp() {
            let (tx, rx) = unbounded_channel();
            let chain_spec = MAINNET.clone();
            let client = Arc::new(MockEthProvider::default());
            let engine = EngineApi {
                client: client.clone(),
                chain_spec: chain_spec.clone(),
                local_store: Default::default(),
                rx: UnboundedReceiverStream::new(rx),
            };

            tokio::spawn(engine);

            let (result_tx, result_rx) = oneshot::channel();
            let block_timestamp = 100;
            let parent_timestamp = block_timestamp + 10;
            let parent = transform_block(random_block(100, None, None, Some(0)), |mut b| {
                b.header.timestamp = parent_timestamp;
                b.header.difficulty =
                    chain_spec.paris_status().terminal_total_difficulty().unwrap() + U256::from(1);
                b
            });
            let block =
                transform_block(random_block(101, Some(parent.hash()), None, Some(0)), |mut b| {
                    b.header.timestamp = block_timestamp;
                    b
                });

            client.add_block(parent.hash(), parent.clone().unseal());

            tx.send(EngineApiMessage::NewPayload(block.clone().into(), result_tx))
                .expect("failed to send engine msg");

            let result = result_rx.await;
            assert_matches!(result, Ok(Ok(_)));
            let expected_result = PayloadStatus::from_status(PayloadStatusEnum::Invalid {
                validation_error: EngineApiError::PayloadTimestamp {
                    invalid: block_timestamp,
                    latest: parent_timestamp,
                }
                .to_string(),
            });
            assert_eq!(result.unwrap().unwrap(), expected_result);
        }

        // TODO: add execution tests
    }

    // non exhaustive tests for engine_getPayload
    // TODO: amend when block building is implemented
    mod get_payload {
        use reth_primitives::MAINNET;

        use super::*;

        #[tokio::test]
        async fn payload_unknown() {
            let (tx, rx) = unbounded_channel();
            let engine = EngineApi {
                client: Arc::new(MockEthProvider::default()),
                chain_spec: MAINNET.clone(),
                local_store: Default::default(),
                rx: UnboundedReceiverStream::new(rx),
            };

            tokio::spawn(engine);

            let payload_id = H64::random();

            let (result_tx, result_rx) = oneshot::channel();
            tx.send(EngineApiMessage::GetPayload(payload_id, result_tx))
                .expect("failed to send engine msg");

            assert_matches!(result_rx.await, Ok(Err(EngineApiError::PayloadUnknown)));
        }
    }

    // https://github.com/ethereum/execution-apis/blob/main/src/engine/paris.md#specification-3
    mod exchange_transition_configuration {
        use reth_primitives::MAINNET;

        use super::*;

        #[tokio::test]
        async fn terminal_td_mismatch() {
            let (tx, rx) = unbounded_channel();
            let chain_spec = MAINNET.clone();
            let engine = EngineApi {
                client: Arc::new(MockEthProvider::default()),
                chain_spec: chain_spec.clone(),
                local_store: Default::default(),
                rx: UnboundedReceiverStream::new(rx),
            };

            tokio::spawn(engine);

            let transition_config = TransitionConfiguration {
                terminal_total_difficulty: chain_spec
                    .paris_status()
                    .terminal_total_difficulty()
                    .unwrap() +
                    U256::from(1),
                ..Default::default()
            };

            let (result_tx, result_rx) = oneshot::channel();
            tx.send(EngineApiMessage::ExchangeTransitionConfiguration(
                transition_config.clone(),
                result_tx,
            ))
            .expect("failed to send engine msg");

            assert_matches!(
                result_rx.await,
                Ok(Err(EngineApiError::TerminalTD { execution, consensus }))
                    if execution == chain_spec.paris_status().terminal_total_difficulty().unwrap()
                        && consensus == U256::from(transition_config.terminal_total_difficulty)
            );
        }

        #[tokio::test]
        async fn terminal_block_hash_mismatch() {
            let (tx, rx) = unbounded_channel();
            let client = Arc::new(MockEthProvider::default());
            let chain_spec = MAINNET.clone();
            let engine = EngineApi {
                client: client.clone(),
                chain_spec: chain_spec.clone(),
                local_store: Default::default(),
                rx: UnboundedReceiverStream::new(rx),
            };

            tokio::spawn(engine);

            let terminal_block_number = 1000;
            let consensus_terminal_block = random_block(terminal_block_number, None, None, None);
            let execution_terminal_block = random_block(terminal_block_number, None, None, None);

            let transition_config = TransitionConfiguration {
                terminal_total_difficulty: chain_spec
                    .paris_status()
                    .terminal_total_difficulty()
                    .unwrap(),
                terminal_block_hash: consensus_terminal_block.hash(),
                terminal_block_number: terminal_block_number.into(),
            };

            // Unknown block number
            let (result_tx, result_rx) = oneshot::channel();
            tx.send(EngineApiMessage::ExchangeTransitionConfiguration(
                transition_config.clone(),
                result_tx,
            ))
            .expect("failed to send engine msg");

            assert_matches!(
                result_rx.await,
                Ok(Err(EngineApiError::TerminalBlockHash { execution, consensus }))
                    if execution.is_none()
                        && consensus == transition_config.terminal_block_hash
            );

            // Add block and to provider local store and test for mismatch
            client.add_block(
                execution_terminal_block.hash(),
                execution_terminal_block.clone().unseal(),
            );

            let (result_tx, result_rx) = oneshot::channel();
            tx.send(EngineApiMessage::ExchangeTransitionConfiguration(
                transition_config.clone(),
                result_tx,
            ))
            .expect("failed to send engine msg");

            assert_matches!(
                result_rx.await,
                Ok(Err(EngineApiError::TerminalBlockHash { execution, consensus }))
                    if execution == Some(execution_terminal_block.hash())
                        && consensus == transition_config.terminal_block_hash
            );
        }

        #[tokio::test]
        async fn configurations_match() {
            let (tx, rx) = unbounded_channel();
            let client = Arc::new(MockEthProvider::default());
            let chain_spec = MAINNET.clone();
            let engine = EngineApi {
                client: client.clone(),
                chain_spec: chain_spec.clone(),
                local_store: Default::default(),
                rx: UnboundedReceiverStream::new(rx),
            };

            tokio::spawn(engine);

            let terminal_block_number = 1000;
            let terminal_block = random_block(terminal_block_number, None, None, None);

            let transition_config = TransitionConfiguration {
                terminal_total_difficulty: chain_spec
                    .paris_status()
                    .terminal_total_difficulty()
                    .unwrap(),
                terminal_block_hash: terminal_block.hash(),
                terminal_block_number: terminal_block_number.into(),
            };

            client.add_block(terminal_block.hash(), terminal_block.clone().unseal());

            let (result_tx, result_rx) = oneshot::channel();
            tx.send(EngineApiMessage::ExchangeTransitionConfiguration(
                transition_config.clone(),
                result_tx,
            ))
            .expect("failed to send engine msg");

            assert_matches!(
                result_rx.await,
                Ok(Ok(config)) if config == transition_config
            );
        }
    }
}

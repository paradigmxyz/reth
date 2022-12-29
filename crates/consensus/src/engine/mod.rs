use futures::StreamExt;
use reth_executor::{
    executor,
    revm_wrap::{State, SubState},
};
use reth_interfaces::consensus::ForkchoiceState;
use reth_primitives::{
    proofs::{self, EMPTY_LIST_HASH},
    rpc::BlockId,
    Header, SealedBlock, TransactionSigned, H64,
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

use crate::Config;

mod error;
pub use error::{EngineApiError, EngineApiResult};

/// The Engine API response sender
pub type EngineApiSender<Ok> = oneshot::Sender<EngineApiResult<Ok>>;

/// Consensus engine API trait.
pub trait ConsensusEngine {
    /// Retrieves payload from local cache.
    fn get_payload(&self, payload_id: H64) -> Option<ExecutionPayload>;

    /// Receives a payload to validate and execute.
    fn new_payload(&mut self, payload: ExecutionPayload) -> EngineApiResult<PayloadStatus>;

    /// Updates the fork choice state
    fn fork_choice_updated(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> EngineApiResult<ForkchoiceUpdated>;

    /// Verifies the transition configuration between execution and consensus clients.
    fn exchange_transition_configuration(
        &self,
        config: TransitionConfiguration,
    ) -> EngineApiResult<TransitionConfiguration>;
}

/// Message type for communicating with [EthConsensusEngine]
#[derive(Debug)]
pub enum EngineMessage {
    /// New payload message
    NewPayload(ExecutionPayload, EngineApiSender<PayloadStatus>),
    /// Get payload message
    GetPayload(H64, EngineApiSender<ExecutionPayload>),
    /// Forkchoice updated message
    ForkchoiceUpdated(
        ForkchoiceState,
        Option<PayloadAttributes>,
        EngineApiSender<ForkchoiceUpdated>,
    ),
    /// Exchange transition configuration message
    ExchangeTransitionConfiguration(
        TransitionConfiguration,
        EngineApiSender<TransitionConfiguration>,
    ),
}

/// The consensus engine API implementation
#[must_use = "EthConsensusEngine does nothing unless polled."]
pub struct EthConsensusEngine<Client> {
    client: Arc<Client>,
    /// Consensus configuration
    config: Config,
    rx: UnboundedReceiverStream<EngineMessage>,
    // TODO: Placeholder for storing future blocks. Make cache bounded.
    // Use [lru](https://crates.io/crates/lru) crate
    local_store: HashMap<H64, ExecutionPayload>,
    // remote_store: HashMap<H64, ExecutionPayload>,
}

impl<Client: HeaderProvider + BlockProvider + StateProvider> EthConsensusEngine<Client> {
    fn on_message(&mut self, msg: EngineMessage) {
        match msg {
            EngineMessage::GetPayload(payload_id, tx) => {
                // NOTE: Will always result in `PayloadUnknown` since we don't support block
                // building for now.
                let _ = tx.send(self.get_payload(payload_id).ok_or(EngineApiError::PayloadUnknown));
            }
            EngineMessage::NewPayload(payload, tx) => {
                let _ = tx.send(self.new_payload(payload));
            }
            EngineMessage::ForkchoiceUpdated(state, attrs, tx) => {
                let _ = tx.send(self.fork_choice_updated(state, attrs));
            }
            EngineMessage::ExchangeTransitionConfiguration(config, tx) => {
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
    /// Ref: https://github.com/ethereum/go-ethereum/blob/79a478bb6176425c2400e949890e668a3d9a3d05/core/beacon/types.go#L145
    fn try_construct_block(&self, payload: ExecutionPayload) -> EngineApiResult<SealedBlock> {
        if payload.extra_data.len() > 32 {
            return Err(EngineApiError::PayloadExtraData(payload.extra_data))
        }

        if payload.base_fee_per_gas.is_zero() {
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
            base_fee_per_gas: Some(payload.base_fee_per_gas.as_u64()),
            extra_data: payload.extra_data.0,
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
}

impl<Client: HeaderProvider + BlockProvider + StateProvider> ConsensusEngine
    for EthConsensusEngine<Client>
{
    fn get_payload(&self, payload_id: H64) -> Option<ExecutionPayload> {
        self.local_store.get(&payload_id).cloned()
    }

    fn new_payload(&mut self, payload: ExecutionPayload) -> EngineApiResult<PayloadStatus> {
        let block = match self.try_construct_block(payload) {
            Ok(b) => b,
            Err(err) => {
                return Ok(PayloadStatus::from_status(PayloadStatusEnum::InvalidBlockHash {
                    validation_error: err.to_string(),
                }))
            }
        };

        // The block already exists in our database
        if self.client.is_known(&block.hash())? {
            return Ok(PayloadStatus::new(PayloadStatusEnum::Valid, block.hash()))
        }

        let Some(parent) = self.client.block(BlockId::Hash(block.parent_hash))? else {
             // TODO: cache block for storing later
             return Ok(PayloadStatus::from_status(PayloadStatusEnum::Syncing))
        };

        let parent_td = self.client.header_td(&block.parent_hash)?;
        if parent_td.unwrap_or_default() <= self.config.merge_terminal_total_difficulty.into() {
            return Ok(PayloadStatus::from_status(PayloadStatusEnum::Invalid {
                validation_error: EngineApiError::PayloadPreMerge.to_string(),
            }))
        }

        if block.timestamp <= parent.timestamp {
            return Err(EngineApiError::PayloadTimestamp {
                invalid: block.timestamp,
                latest: parent.timestamp,
            })
        }

        let (header, body, _) = block.split();
        let transactions = body
            .into_iter()
            .map(|tx| {
                let tx_hash = tx.hash;
                tx.into_ecrecovered().ok_or(EngineApiError::PayloadSignerRecovery { hash: tx_hash })
            })
            .collect::<Result<Vec<_>, EngineApiError>>()?;
        let state_provider = SubState::new(State::new(&*self.client));
        let config = (&self.config).into();
        match executor::execute_and_verify_receipt(
            &header,
            &transactions,
            &[],
            &config,
            state_provider,
        ) {
            Ok(_) => Ok(PayloadStatus::new(PayloadStatusEnum::Valid, header.hash())),
            Err(err) => Ok(PayloadStatus::new(
                PayloadStatusEnum::Invalid { validation_error: err.to_string() },
                header.parent_hash, // The parent hash is already in our database hence it is valid
            )),
        }
    }

    fn fork_choice_updated(
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

    fn exchange_transition_configuration(
        &self,
        config: TransitionConfiguration,
    ) -> EngineApiResult<TransitionConfiguration> {
        let TransitionConfiguration {
            terminal_total_difficulty,
            terminal_block_hash,
            terminal_block_number,
        } = config;

        // Compare total difficulty values
        let merge_terminal_td = self.config.merge_terminal_total_difficulty.into();
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
        let local_hash = self.client.block_hash(terminal_block_number.into())?;

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

impl<Client> Future for EthConsensusEngine<Client>
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

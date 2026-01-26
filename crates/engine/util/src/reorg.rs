//! Stream wrapper that simulates reorgs.

use alloy_consensus::{BlockHeader, Transaction};
use alloy_primitives::B256;
use alloy_rpc_types_engine::{ForkchoiceState, PayloadStatus};
use futures::{stream::FuturesUnordered, Stream, StreamExt, TryFutureExt};
use itertools::Either;
use reth_chainspec::{ChainSpecProvider, EthChainSpec};
use reth_engine_primitives::{
    BeaconEngineMessage, BeaconOnNewPayloadError, ExecutionPayload as _, OnForkChoiceUpdated,
};
use reth_engine_tree::tree::EngineValidator;
use reth_errors::{BlockExecutionError, BlockValidationError, RethError, RethResult};
use reth_evm::{
    execute::{BlockBuilder, BlockBuilderOutcome},
    ConfigureEvm,
};
use reth_payload_primitives::{BuiltPayload, EngineApiMessageVersion, PayloadTypes};
use reth_primitives_traits::{
    block::Block as _, BlockBody as _, BlockTy, HeaderTy, SignedTransaction,
};
use reth_revm::{database::StateProviderDatabase, db::State};
use reth_storage_api::{errors::ProviderError, BlockReader, StateProviderFactory};
use std::{
    collections::VecDeque,
    fmt,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::{sync::oneshot, task::JoinHandle};
use tracing::*;

#[derive(Debug)]
enum EngineReorgState<T: PayloadTypes> {
    Forward,
    Reorg { queue: VecDeque<BeaconEngineMessage<T>> },
}

type EngineReorgResponse = Result<
    Either<Result<PayloadStatus, BeaconOnNewPayloadError>, RethResult<OnForkChoiceUpdated>>,
    oneshot::error::RecvError,
>;

type ReorgResponseFut = Pin<Box<dyn Future<Output = EngineReorgResponse> + Send + Sync>>;

/// Output from the reorg head creation task.
struct ReorgHeadOutput<T: PayloadTypes> {
    /// The reorg block converted to execution data.
    reorg_payload: T::ExecutionData,
    /// The hash of the reorg block for the forkchoice state.
    reorg_block_hash: B256,
}

/// Context stored while waiting for a reorg block to be built in a background task.
struct PendingReorgHead<T: PayloadTypes> {
    /// Handle to the blocking task building the reorg block.
    task: JoinHandle<RethResult<ReorgHeadOutput<T>>>,
    /// Original payload to forward if reorg fails.
    payload: T::ExecutionData,
    /// Channel to send response for original payload.
    tx: oneshot::Sender<Result<PayloadStatus, BeaconOnNewPayloadError>>,
    /// Last forkchoice state for building reorg FCU message.
    last_forkchoice_state: ForkchoiceState,
}

/// Engine API stream wrapper that simulates reorgs with specified frequency.
///
/// Note: The `Provider`, `Evm`, and `Validator` are wrapped in `Arc` internally to enable
/// cloning for the background blocking task that builds reorg blocks.
#[pin_project::pin_project]
pub struct EngineReorg<S, T: PayloadTypes, Provider, Evm, Validator> {
    /// Underlying stream
    #[pin]
    stream: S,
    /// Database provider.
    provider: Arc<Provider>,
    /// Evm configuration.
    evm_config: Arc<Evm>,
    /// Payload validator.
    payload_validator: Arc<Validator>,
    /// The frequency of reorgs.
    frequency: usize,
    /// The depth of reorgs.
    depth: usize,
    /// The number of forwarded forkchoice states.
    /// This is reset after a reorg.
    forkchoice_states_forwarded: usize,
    /// Current state of the stream.
    state: EngineReorgState<T>,
    /// Last forkchoice state.
    last_forkchoice_state: Option<ForkchoiceState>,
    /// Pending engine responses to reorg messages.
    reorg_responses: FuturesUnordered<ReorgResponseFut>,
    /// Pending reorg head computation running in a background task.
    pending_reorg: Option<PendingReorgHead<T>>,
}

impl<S, T: PayloadTypes, Provider, Evm, Validator> EngineReorg<S, T, Provider, Evm, Validator> {
    /// Creates new [`EngineReorg`] stream wrapper.
    pub fn new(
        stream: S,
        provider: Provider,
        evm_config: Evm,
        payload_validator: Validator,
        frequency: usize,
        depth: usize,
    ) -> Self {
        Self {
            stream,
            provider: Arc::new(provider),
            evm_config: Arc::new(evm_config),
            payload_validator: Arc::new(payload_validator),
            frequency,
            depth,
            state: EngineReorgState::Forward,
            forkchoice_states_forwarded: 0,
            last_forkchoice_state: None,
            reorg_responses: FuturesUnordered::new(),
            pending_reorg: None,
        }
    }
}

impl<S, T: PayloadTypes, Provider, Evm, Validator> fmt::Debug
    for EngineReorg<S, T, Provider, Evm, Validator>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EngineReorg")
            .field("frequency", &self.frequency)
            .field("depth", &self.depth)
            .field("forkchoice_states_forwarded", &self.forkchoice_states_forwarded)
            .field("state", &self.state)
            .field("last_forkchoice_state", &self.last_forkchoice_state)
            .field("pending_reorg", &self.pending_reorg.is_some())
            .finish_non_exhaustive()
    }
}

impl<S, T, Provider, Evm, Validator> Stream for EngineReorg<S, T, Provider, Evm, Validator>
where
    S: Stream<Item = BeaconEngineMessage<T>>,
    T: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = Evm::Primitives>>,
    Provider: BlockReader<Header = HeaderTy<Evm::Primitives>, Block = BlockTy<Evm::Primitives>>
        + StateProviderFactory
        + ChainSpecProvider
        + 'static,
    Evm: ConfigureEvm + 'static,
    Validator: EngineValidator<T, Evm::Primitives> + 'static,
{
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        loop {
            // Poll pending reorg task if present.
            if let Some(pending) = this.pending_reorg.as_mut() {
                match Pin::new(&mut pending.task).poll(cx) {
                    Poll::Ready(Ok(Ok(reorg_output))) => {
                        let pending = this.pending_reorg.take().unwrap();
                        let reorg_forkchoice_state = ForkchoiceState {
                            finalized_block_hash: pending
                                .last_forkchoice_state
                                .finalized_block_hash,
                            safe_block_hash: pending.last_forkchoice_state.safe_block_hash,
                            head_block_hash: reorg_output.reorg_block_hash,
                        };

                        let (reorg_payload_tx, reorg_payload_rx) = oneshot::channel();
                        let (reorg_fcu_tx, reorg_fcu_rx) = oneshot::channel();
                        this.reorg_responses.extend([
                            Box::pin(reorg_payload_rx.map_ok(Either::Left)) as ReorgResponseFut,
                            Box::pin(reorg_fcu_rx.map_ok(Either::Right)) as ReorgResponseFut,
                        ]);

                        let queue = VecDeque::from([
                            // Current payload
                            BeaconEngineMessage::NewPayload {
                                payload: pending.payload,
                                tx: pending.tx,
                            },
                            // Reorg payload
                            BeaconEngineMessage::NewPayload {
                                payload: reorg_output.reorg_payload,
                                tx: reorg_payload_tx,
                            },
                            // Reorg forkchoice state
                            BeaconEngineMessage::ForkchoiceUpdated {
                                state: reorg_forkchoice_state,
                                payload_attrs: None,
                                tx: reorg_fcu_tx,
                                version: EngineApiMessageVersion::default(),
                            },
                        ]);
                        *this.state = EngineReorgState::Reorg { queue };
                        continue
                    }
                    Poll::Ready(Ok(Err(error))) => {
                        let pending = this.pending_reorg.take().unwrap();
                        error!(target: "engine::stream::reorg", %error, "Error creating reorg head");
                        return Poll::Ready(Some(BeaconEngineMessage::NewPayload {
                            payload: pending.payload,
                            tx: pending.tx,
                        }))
                    }
                    Poll::Ready(Err(join_error)) => {
                        let pending = this.pending_reorg.take().unwrap();
                        error!(target: "engine::stream::reorg", %join_error, "Reorg task panicked");
                        return Poll::Ready(Some(BeaconEngineMessage::NewPayload {
                            payload: pending.payload,
                            tx: pending.tx,
                        }))
                    }
                    Poll::Pending => return Poll::Pending,
                }
            }

            if let Poll::Ready(Some(response)) = this.reorg_responses.poll_next_unpin(cx) {
                match response {
                    Ok(Either::Left(Ok(payload_status))) => {
                        debug!(target: "engine::stream::reorg", ?payload_status, "Received response for reorg new payload");
                    }
                    Ok(Either::Left(Err(payload_error))) => {
                        error!(target: "engine::stream::reorg", %payload_error, "Error on reorg new payload");
                    }
                    Ok(Either::Right(Ok(fcu_status))) => {
                        debug!(target: "engine::stream::reorg", ?fcu_status, "Received response for reorg forkchoice update");
                    }
                    Ok(Either::Right(Err(fcu_error))) => {
                        error!(target: "engine::stream::reorg", %fcu_error, "Error on reorg forkchoice update");
                    }
                    Err(_) => {}
                };
                continue
            }

            if let EngineReorgState::Reorg { queue } = &mut this.state {
                match queue.pop_front() {
                    Some(msg) => return Poll::Ready(Some(msg)),
                    None => {
                        *this.forkchoice_states_forwarded = 0;
                        *this.state = EngineReorgState::Forward;
                    }
                }
            }

            let next = ready!(this.stream.poll_next_unpin(cx));
            let item = match (next, &this.last_forkchoice_state) {
                (
                    Some(BeaconEngineMessage::NewPayload { payload, tx }),
                    Some(last_forkchoice_state),
                ) if this.forkchoice_states_forwarded > this.frequency &&
                        // Only enter reorg state if new payload attaches to current head.
                        last_forkchoice_state.head_block_hash == payload.parent_hash() =>
                {
                    // Enter the reorg state.
                    // The current payload will be immediately forwarded by being in front of the
                    // queue. Then we attempt to reorg the current head by generating a payload that
                    // attaches to the head's parent and is based on the non-conflicting
                    // transactions (txs from block `n + 1` that are valid at block `n` according to
                    // consensus checks) from the current payload as well as the corresponding
                    // forkchoice state. We will rely on CL to reorg us back to canonical chain.

                    // Spawn blocking task to build the reorg head.
                    let provider = Arc::clone(this.provider);
                    let evm_config = Arc::clone(this.evm_config);
                    let payload_validator = Arc::clone(this.payload_validator);
                    let depth = *this.depth;
                    let payload_clone = payload.clone();

                    let task = tokio::task::spawn_blocking(move || {
                        create_reorg_head::<_, _, T, _>(
                            provider,
                            evm_config,
                            payload_validator,
                            depth,
                            payload_clone,
                        )
                    });

                    *this.pending_reorg = Some(PendingReorgHead {
                        task,
                        payload,
                        tx,
                        last_forkchoice_state: *last_forkchoice_state,
                    });
                    continue
                }
                (
                    Some(BeaconEngineMessage::ForkchoiceUpdated {
                        state,
                        payload_attrs,
                        tx,
                        version,
                    }),
                    _,
                ) => {
                    // Record last forkchoice state forwarded to the engine.
                    // We do not care if it's valid since engine should be able to handle
                    // reorgs that rely on invalid forkchoice state.
                    *this.last_forkchoice_state = Some(state);
                    *this.forkchoice_states_forwarded += 1;
                    Some(BeaconEngineMessage::ForkchoiceUpdated {
                        state,
                        payload_attrs,
                        tx,
                        version,
                    })
                }
                (item, _) => item,
            };
            return Poll::Ready(item)
        }
    }
}

fn create_reorg_head<Provider, Evm, T, Validator>(
    provider: Arc<Provider>,
    evm_config: Arc<Evm>,
    payload_validator: Arc<Validator>,
    mut depth: usize,
    next_payload: T::ExecutionData,
) -> RethResult<ReorgHeadOutput<T>>
where
    Provider: BlockReader<Header = HeaderTy<Evm::Primitives>, Block = BlockTy<Evm::Primitives>>
        + StateProviderFactory
        + ChainSpecProvider<ChainSpec: EthChainSpec>,
    Evm: ConfigureEvm,
    T: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = Evm::Primitives>>,
    Validator: EngineValidator<T, Evm::Primitives>,
{
    // Ensure next payload is valid.
    let next_block =
        payload_validator.convert_payload_to_block(next_payload).map_err(RethError::msg)?;

    // Fetch reorg target block depending on its depth and its parent.
    let mut previous_hash = next_block.parent_hash();
    let mut candidate_transactions = next_block.into_body().transactions().to_vec();
    let reorg_target = 'target: {
        loop {
            let reorg_target = provider
                .block_by_hash(previous_hash)?
                .ok_or_else(|| ProviderError::HeaderNotFound(previous_hash.into()))?;
            if depth == 0 {
                break 'target reorg_target.seal_slow()
            }

            depth -= 1;
            previous_hash = reorg_target.header().parent_hash();
            candidate_transactions = reorg_target.into_body().into_transactions();
        }
    };
    let reorg_target_parent = provider
        .sealed_header_by_hash(reorg_target.header().parent_hash())?
        .ok_or_else(|| ProviderError::HeaderNotFound(reorg_target.header().parent_hash().into()))?;

    debug!(target: "engine::stream::reorg", number = reorg_target.header().number(), hash = %previous_hash, "Selected reorg target");

    // Configure state
    let state_provider = provider.state_by_block_hash(reorg_target.header().parent_hash())?;
    let mut state = State::builder()
        .with_database_ref(StateProviderDatabase::new(&state_provider))
        .with_bundle_update()
        .build();

    let ctx = evm_config.context_for_block(&reorg_target).map_err(RethError::other)?;
    let evm = evm_config.evm_for_block(&mut state, &reorg_target).map_err(RethError::other)?;
    let mut builder = evm_config.create_block_builder(evm, &reorg_target_parent, ctx);

    builder.apply_pre_execution_changes()?;

    let mut cumulative_gas_used = 0;
    for tx in candidate_transactions {
        // ensure we still have capacity for this transaction
        if cumulative_gas_used + tx.gas_limit() > reorg_target.gas_limit() {
            continue
        }

        let tx_recovered =
            tx.try_into_recovered().map_err(|_| ProviderError::SenderRecoveryError)?;
        let gas_used = match builder.execute_transaction(tx_recovered) {
            Ok(gas_used) => gas_used,
            Err(BlockExecutionError::Validation(BlockValidationError::InvalidTx {
                hash,
                error,
            })) => {
                trace!(target: "engine::stream::reorg", hash = %hash, ?error, "Error executing transaction from next block");
                continue
            }
            // Treat error as fatal
            Err(error) => return Err(RethError::Execution(error)),
        };

        cumulative_gas_used += gas_used;
    }

    let BlockBuilderOutcome { block, .. } = builder.finish(&state_provider)?;

    let sealed_block = block.into_sealed_block();
    let reorg_block_hash = sealed_block.hash();
    let reorg_payload = T::block_to_payload(sealed_block);

    Ok(ReorgHeadOutput { reorg_payload, reorg_block_hash })
}

//! Stream wrapper that simulates reorgs.

use alloy_consensus::{BlockHeader, Transaction};
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
use reth_payload_primitives::{BuiltPayload, PayloadTypes};
use reth_primitives_traits::{
    block::Block as _, BlockBody as _, BlockTy, HeaderTy, SealedBlock, SignedTransaction,
};
use reth_revm::{database::StateProviderDatabase, db::State};
use reth_storage_api::{errors::ProviderError, BlockReader, StateProviderFactory};
use std::{
    collections::VecDeque,
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
type PendingReorgTask<T> =
    JoinHandle<RethResult<(<T as PayloadTypes>::ExecutionData, ForkchoiceState)>>;

/// Engine API stream wrapper that simulates reorgs with specified frequency.
#[derive(Debug)]
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
    /// In-flight reorg head construction task.
    pending_reorg_task: Option<PendingReorgTask<T>>,
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
            pending_reorg_task: None,
        }
    }
}

impl<S, T, Provider, Evm, Validator> Stream for EngineReorg<S, T, Provider, Evm, Validator>
where
    S: Stream<Item = BeaconEngineMessage<T>>,
    T: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = Evm::Primitives>>,
    Provider: BlockReader<Header = HeaderTy<Evm::Primitives>, Block = BlockTy<Evm::Primitives>>
        + StateProviderFactory
        + ChainSpecProvider
        + Send
        + Sync
        + 'static,
    Evm: ConfigureEvm + Send + Sync + 'static,
    Validator: EngineValidator<T, Evm::Primitives> + Send + Sync + 'static,
    T::ExecutionData: Send + 'static,
{
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        loop {
            if let Some(pending_reorg_task) = this.pending_reorg_task.as_mut() {
                match Pin::new(pending_reorg_task).poll(cx) {
                    Poll::Ready(join_result) => {
                        this.pending_reorg_task.take();
                        match join_result {
                            Ok(Ok((reorg_payload, reorg_forkchoice_state))) => {
                                let (reorg_payload_tx, reorg_payload_rx) = oneshot::channel();
                                let (reorg_fcu_tx, reorg_fcu_rx) = oneshot::channel();
                                this.reorg_responses.extend([
                                    Box::pin(reorg_payload_rx.map_ok(Either::Left))
                                        as ReorgResponseFut,
                                    Box::pin(reorg_fcu_rx.map_ok(Either::Right))
                                        as ReorgResponseFut,
                                ]);

                                let queue = VecDeque::from([
                                    // Reorg payload
                                    BeaconEngineMessage::NewPayload {
                                        payload: reorg_payload,
                                        tx: reorg_payload_tx,
                                    },
                                    // Reorg forkchoice state
                                    BeaconEngineMessage::ForkchoiceUpdated {
                                        state: reorg_forkchoice_state,
                                        payload_attrs: None,
                                        tx: reorg_fcu_tx,
                                    },
                                ]);
                                *this.state = EngineReorgState::Reorg { queue };
                                continue
                            }
                            Ok(Err(error)) => {
                                error!(target: "engine::stream::reorg", %error, "Error attempting to create reorg head");
                            }
                            Err(error) => {
                                error!(target: "engine::stream::reorg", %error, "Reorg head task failed");
                            }
                        }
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
                ) if should_start_reorg(
                    *this.forkchoice_states_forwarded,
                    *this.frequency,
                    last_forkchoice_state.head_block_hash,
                    payload.parent_hash(),
                ) =>
                {
                    // Enter the reorg state.
                    // The current payload is immediately forwarded. Then we attempt to reorg the
                    // current head by generating a payload that
                    // attaches to the head's parent and is based on the non-conflicting
                    // transactions (txs from block `n + 1` that are valid at block `n` according to
                    // consensus checks) from the current payload as well as the corresponding
                    // forkchoice state. We will rely on CL to reorg us back to canonical chain.
                    let provider = Arc::clone(this.provider);
                    let evm_config = Arc::clone(this.evm_config);
                    let payload_validator = Arc::clone(this.payload_validator);
                    let depth = *this.depth;
                    let next_payload = payload.clone();
                    let last_forkchoice_state = *last_forkchoice_state;
                    let task = tokio::task::spawn_blocking(move || {
                        let reorg_block = create_reorg_head(
                            provider.as_ref(),
                            evm_config.as_ref(),
                            payload_validator.as_ref(),
                            depth,
                            next_payload,
                        )?;
                        let reorg_forkchoice_state = ForkchoiceState {
                            finalized_block_hash: last_forkchoice_state.finalized_block_hash,
                            safe_block_hash: last_forkchoice_state.safe_block_hash,
                            head_block_hash: reorg_block.hash(),
                        };

                        Ok((T::block_to_payload(reorg_block), reorg_forkchoice_state))
                    });

                    *this.pending_reorg_task = Some(task);
                    return Poll::Ready(Some(BeaconEngineMessage::NewPayload { payload, tx }))
                }
                (Some(BeaconEngineMessage::ForkchoiceUpdated { state, payload_attrs, tx }), _) => {
                    // Record last forkchoice state forwarded to the engine.
                    // We do not care if it's valid since engine should be able to handle
                    // reorgs that rely on invalid forkchoice state.
                    *this.last_forkchoice_state = Some(state);
                    *this.forkchoice_states_forwarded += 1;
                    Some(BeaconEngineMessage::ForkchoiceUpdated { state, payload_attrs, tx })
                }
                (item, _) => item,
            };
            return Poll::Ready(item)
        }
    }
}

fn create_reorg_head<Provider, Evm, T, Validator>(
    provider: &Provider,
    evm_config: &Evm,
    payload_validator: &Validator,
    mut depth: usize,
    next_payload: T::ExecutionData,
) -> RethResult<SealedBlock<BlockTy<Evm::Primitives>>>
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

    let BlockBuilderOutcome { block, .. } = builder.finish(&state_provider, None)?;

    Ok(block.into_sealed_block())
}

#[inline]
fn should_start_reorg<Hash: Eq>(
    forkchoice_states_forwarded: usize,
    frequency: usize,
    last_head_hash: Hash,
    payload_parent_hash: Hash,
) -> bool {
    forkchoice_states_forwarded > frequency && last_head_hash == payload_parent_hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_not_start_reorg_when_frequency_not_exceeded() {
        let hash = [1u8; 32];
        assert!(!should_start_reorg(3, 3, hash, hash));
    }

    #[test]
    fn should_not_start_reorg_when_parent_does_not_match_head() {
        assert!(!should_start_reorg(4, 3, [1u8; 32], [2u8; 32]));
    }

    #[test]
    fn should_start_reorg_when_frequency_exceeded_and_parent_matches() {
        let hash = [7u8; 32];
        assert!(should_start_reorg(4, 3, hash, hash));
    }
}

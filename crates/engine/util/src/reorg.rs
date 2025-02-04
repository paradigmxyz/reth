//! Stream wrapper that simulates reorgs.

use alloy_consensus::{Header, Transaction};
use alloy_rpc_types_engine::{
    CancunPayloadFields, ExecutionPayload, ExecutionPayloadSidecar, ForkchoiceState, PayloadStatus,
};
use futures::{stream::FuturesUnordered, Stream, StreamExt, TryFutureExt};
use itertools::Either;
use reth_chainspec::EthChainSpec;
use reth_engine_primitives::{
    BeaconEngineMessage, BeaconOnNewPayloadError, EngineTypes, ExecutionData,
    ExecutionPayload as _, OnForkChoiceUpdated,
};
use reth_errors::{BlockExecutionError, BlockValidationError, RethError, RethResult};
use reth_ethereum_forks::EthereumHardforks;
use reth_evm::{
    state_change::post_block_withdrawals_balance_increments, system_calls::SystemCaller,
    ConfigureEvm, Evm, EvmError,
};
use reth_payload_primitives::EngineApiMessageVersion;
use reth_payload_validator::ExecutionPayloadValidator;
use reth_primitives::{transaction::SignedTransactionIntoRecoveredExt, Block, BlockBody, Receipt};
use reth_primitives_traits::{block::Block as _, proofs, SignedTransaction};
use reth_provider::{BlockReader, ExecutionOutcome, ProviderError, StateProviderFactory};
use reth_revm::{
    database::StateProviderDatabase,
    db::{states::bundle_state::BundleRetention, State},
    DatabaseCommit,
};
use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::sync::oneshot;
use tracing::*;

#[derive(Debug)]
enum EngineReorgState<Engine: EngineTypes> {
    Forward,
    Reorg { queue: VecDeque<BeaconEngineMessage<Engine>> },
}

type EngineReorgResponse = Result<
    Either<Result<PayloadStatus, BeaconOnNewPayloadError>, RethResult<OnForkChoiceUpdated>>,
    oneshot::error::RecvError,
>;

type ReorgResponseFut = Pin<Box<dyn Future<Output = EngineReorgResponse> + Send + Sync>>;

/// Engine API stream wrapper that simulates reorgs with specified frequency.
#[derive(Debug)]
#[pin_project::pin_project]
pub struct EngineReorg<S, Engine: EngineTypes, Provider, Evm, Spec> {
    /// Underlying stream
    #[pin]
    stream: S,
    /// Database provider.
    provider: Provider,
    /// Evm configuration.
    evm_config: Evm,
    /// Payload validator.
    payload_validator: ExecutionPayloadValidator<Spec>,
    /// The frequency of reorgs.
    frequency: usize,
    /// The depth of reorgs.
    depth: usize,
    /// The number of forwarded forkchoice states.
    /// This is reset after a reorg.
    forkchoice_states_forwarded: usize,
    /// Current state of the stream.
    state: EngineReorgState<Engine>,
    /// Last forkchoice state.
    last_forkchoice_state: Option<ForkchoiceState>,
    /// Pending engine responses to reorg messages.
    reorg_responses: FuturesUnordered<ReorgResponseFut>,
}

impl<S, Engine: EngineTypes, Provider, Evm, Spec> EngineReorg<S, Engine, Provider, Evm, Spec> {
    /// Creates new [`EngineReorg`] stream wrapper.
    pub fn new(
        stream: S,
        provider: Provider,
        evm_config: Evm,
        payload_validator: ExecutionPayloadValidator<Spec>,
        frequency: usize,
        depth: usize,
    ) -> Self {
        Self {
            stream,
            provider,
            evm_config,
            payload_validator,
            frequency,
            depth,
            state: EngineReorgState::Forward,
            forkchoice_states_forwarded: 0,
            last_forkchoice_state: None,
            reorg_responses: FuturesUnordered::new(),
        }
    }
}

impl<S, Engine, Provider, Evm, Spec> Stream for EngineReorg<S, Engine, Provider, Evm, Spec>
where
    S: Stream<Item = BeaconEngineMessage<Engine>>,
    Engine: EngineTypes<ExecutionData = ExecutionData>,
    Provider: BlockReader<Block = reth_primitives::Block> + StateProviderFactory,
    Evm: ConfigureEvm<Header = Header, Transaction = reth_primitives::TransactionSigned>,
    Spec: EthChainSpec + EthereumHardforks,
{
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        loop {
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
                    // TODO: This is an expensive blocking operation, ideally it's spawned as a task
                    // so that the stream could yield the control back.
                    let reorg_payload = match create_reorg_head(
                        this.provider,
                        this.evm_config,
                        this.payload_validator,
                        *this.depth,
                        payload.clone(),
                    ) {
                        Ok(result) => result,
                        Err(error) => {
                            error!(target: "engine::stream::reorg", %error, "Error attempting to create reorg head");
                            // Forward the payload and attempt to create reorg on top of
                            // the next one
                            return Poll::Ready(Some(BeaconEngineMessage::NewPayload {
                                payload,
                                tx,
                            }))
                        }
                    };
                    let reorg_forkchoice_state = ForkchoiceState {
                        finalized_block_hash: last_forkchoice_state.finalized_block_hash,
                        safe_block_hash: last_forkchoice_state.safe_block_hash,
                        head_block_hash: reorg_payload.block_hash(),
                    };

                    let (reorg_payload_tx, reorg_payload_rx) = oneshot::channel();
                    let (reorg_fcu_tx, reorg_fcu_rx) = oneshot::channel();
                    this.reorg_responses.extend([
                        Box::pin(reorg_payload_rx.map_ok(Either::Left)) as ReorgResponseFut,
                        Box::pin(reorg_fcu_rx.map_ok(Either::Right)) as ReorgResponseFut,
                    ]);

                    let queue = VecDeque::from([
                        // Current payload
                        BeaconEngineMessage::NewPayload { payload, tx },
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
                            version: EngineApiMessageVersion::default(),
                        },
                    ]);
                    *this.state = EngineReorgState::Reorg { queue };
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

fn create_reorg_head<Provider, Evm, Spec>(
    provider: &Provider,
    evm_config: &Evm,
    payload_validator: &ExecutionPayloadValidator<Spec>,
    mut depth: usize,
    next_payload: ExecutionData,
) -> RethResult<ExecutionData>
where
    Provider: BlockReader<Block = reth_primitives::Block> + StateProviderFactory,
    Evm: ConfigureEvm<Header = Header, Transaction = reth_primitives::TransactionSigned>,
    Spec: EthChainSpec + EthereumHardforks,
{
    let chain_spec = payload_validator.chain_spec();

    // Ensure next payload is valid.
    let next_block =
        payload_validator.ensure_well_formed_payload(next_payload).map_err(RethError::msg)?;

    // Fetch reorg target block depending on its depth and its parent.
    let mut previous_hash = next_block.parent_hash;
    let mut candidate_transactions = next_block.into_body().transactions;
    let reorg_target = 'target: {
        loop {
            let reorg_target = provider
                .block_by_hash(previous_hash)?
                .ok_or_else(|| ProviderError::HeaderNotFound(previous_hash.into()))?;
            if depth == 0 {
                break 'target reorg_target
            }

            depth -= 1;
            previous_hash = reorg_target.parent_hash;
            candidate_transactions = reorg_target.body.transactions;
        }
    };
    let reorg_target_parent = provider
        .block_by_hash(reorg_target.parent_hash)?
        .ok_or_else(|| ProviderError::HeaderNotFound(reorg_target.parent_hash.into()))?;

    debug!(target: "engine::stream::reorg", number = reorg_target.number, hash = %previous_hash, "Selected reorg target");

    // Configure state
    let state_provider = provider.state_by_block_hash(reorg_target.parent_hash)?;
    let mut state = State::builder()
        .with_database_ref(StateProviderDatabase::new(&state_provider))
        .with_bundle_update()
        .build();

    // Configure EVM
    let mut evm = evm_config.evm_for_block(&mut state, &reorg_target.header);

    // apply eip-4788 pre block contract call
    let mut system_caller = SystemCaller::new(evm_config.clone(), chain_spec.clone());

    system_caller.apply_beacon_root_contract_call(
        reorg_target.timestamp,
        reorg_target.number,
        reorg_target.parent_beacon_block_root,
        &mut evm,
    )?;

    let mut cumulative_gas_used = 0;
    let mut sum_blob_gas_used = 0;
    let mut transactions = Vec::new();
    let mut receipts = Vec::new();
    let mut versioned_hashes = Vec::new();
    for tx in candidate_transactions {
        // ensure we still have capacity for this transaction
        if cumulative_gas_used + tx.gas_limit() > reorg_target.gas_limit {
            continue
        }

        // Configure the environment for the block.
        let tx_recovered =
            tx.try_clone_into_recovered().map_err(|_| ProviderError::SenderRecoveryError)?;
        let tx_env = evm_config.tx_env(&tx_recovered, tx_recovered.signer());
        let exec_result = match evm.transact(tx_env) {
            Ok(result) => result,
            Err(err) if err.is_invalid_tx_err() => {
                trace!(target: "engine::stream::reorg", hash = %tx.tx_hash(), ?err, "Error executing transaction from next block");
                continue
            }
            // Treat error as fatal
            Err(error) => {
                return Err(RethError::Execution(BlockExecutionError::Validation(
                    BlockValidationError::EVM { hash: *tx.tx_hash(), error: Box::new(error) },
                )))
            }
        };
        evm.db_mut().commit(exec_result.state);

        if let Some(blob_tx) = tx.as_eip4844() {
            sum_blob_gas_used += blob_tx.blob_gas();
            versioned_hashes.extend(blob_tx.blob_versioned_hashes.clone());
        }

        cumulative_gas_used += exec_result.result.gas_used();
        #[allow(clippy::needless_update)] // side-effect of optimism fields
        receipts.push(Receipt {
            tx_type: tx.tx_type(),
            success: exec_result.result.is_success(),
            cumulative_gas_used,
            logs: exec_result.result.into_logs().into_iter().collect(),
            ..Default::default()
        });

        // append transaction to the list of executed transactions
        transactions.push(tx);
    }
    drop(evm);

    if let Some(withdrawals) = &reorg_target.body.withdrawals {
        state.increment_balances(post_block_withdrawals_balance_increments(
            chain_spec,
            reorg_target.timestamp,
            withdrawals,
        ))?;
    }

    // merge all transitions into bundle state, this would apply the withdrawal balance changes
    // and 4788 contract call
    state.merge_transitions(BundleRetention::PlainState);

    let outcome: ExecutionOutcome = ExecutionOutcome::new(
        state.take_bundle(),
        vec![receipts],
        reorg_target.number,
        Default::default(),
    );
    let hashed_state = state_provider.hashed_post_state(outcome.state());

    let (blob_gas_used, excess_blob_gas) =
        if let Some(blob_params) = chain_spec.blob_params_at_timestamp(reorg_target.timestamp) {
            (Some(sum_blob_gas_used), reorg_target_parent.next_block_excess_blob_gas(blob_params))
        } else {
            (None, None)
        };

    let reorg_block = Block {
        header: Header {
            // Set same fields as the reorg target
            parent_hash: reorg_target.header.parent_hash,
            ommers_hash: reorg_target.header.ommers_hash,
            beneficiary: reorg_target.header.beneficiary,
            difficulty: reorg_target.header.difficulty,
            number: reorg_target.header.number,
            gas_limit: reorg_target.header.gas_limit,
            timestamp: reorg_target.header.timestamp,
            extra_data: reorg_target.header.extra_data,
            mix_hash: reorg_target.header.mix_hash,
            nonce: reorg_target.header.nonce,
            base_fee_per_gas: reorg_target.header.base_fee_per_gas,
            parent_beacon_block_root: reorg_target.header.parent_beacon_block_root,
            withdrawals_root: reorg_target.header.withdrawals_root,

            // Compute or add new fields
            transactions_root: proofs::calculate_transaction_root(&transactions),
            receipts_root: outcome.ethereum_receipts_root(reorg_target.header.number).unwrap(),
            logs_bloom: outcome.block_logs_bloom(reorg_target.header.number).unwrap(),
            gas_used: cumulative_gas_used,
            blob_gas_used,
            excess_blob_gas,
            state_root: state_provider.state_root(hashed_state)?,
            requests_hash: None, // TODO(prague)
        },
        body: BlockBody {
            transactions,
            ommers: reorg_target.body.ommers,
            withdrawals: reorg_target.body.withdrawals,
        },
    }
    .seal_slow();

    Ok(ExecutionData {
        payload: ExecutionPayload::from_block_unchecked(
            reorg_block.hash(),
            &reorg_block.into_block(),
        )
        .0,
        // todo(onbjerg): how do we support execution requests?
        sidecar: reorg_target
            .header
            .parent_beacon_block_root
            .map(|root| {
                ExecutionPayloadSidecar::v3(CancunPayloadFields {
                    parent_beacon_block_root: root,
                    versioned_hashes,
                })
            })
            .unwrap_or_else(ExecutionPayloadSidecar::none),
    })
}

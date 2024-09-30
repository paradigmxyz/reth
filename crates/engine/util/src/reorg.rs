//! Stream wrapper that simulates reorgs.

use alloy_primitives::U256;
use alloy_rpc_types_engine::{
    CancunPayloadFields, ExecutionPayload, ForkchoiceState, PayloadStatus,
};
use futures::{stream::FuturesUnordered, Stream, StreamExt, TryFutureExt};
use itertools::Either;
use reth_beacon_consensus::{BeaconEngineMessage, BeaconOnNewPayloadError, OnForkChoiceUpdated};
use reth_engine_primitives::EngineTypes;
use reth_errors::{BlockExecutionError, BlockValidationError, RethError, RethResult};
use reth_ethereum_forks::EthereumHardforks;
use reth_evm::{system_calls::SystemCaller, ConfigureEvm};
use reth_payload_validator::ExecutionPayloadValidator;
use reth_primitives::{proofs, Block, BlockBody, Header, Receipt, Receipts};
use reth_provider::{BlockReader, ExecutionOutcome, ProviderError, StateProviderFactory};
use reth_revm::{
    database::StateProviderDatabase,
    db::{states::bundle_state::BundleRetention, State},
    state_change::post_block_withdrawals_balance_increments,
    DatabaseCommit,
};
use reth_rpc_types_compat::engine::payload::block_to_payload;
use reth_trie::HashedPostState;
use revm_primitives::{
    calc_excess_blob_gas, BlockEnv, CfgEnvWithHandlerCfg, EVMError, EnvWithHandlerCfg,
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
    Engine: EngineTypes,
    Provider: BlockReader + StateProviderFactory,
    Evm: ConfigureEvm<Header = Header>,
    Spec: EthereumHardforks,
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
                    Some(BeaconEngineMessage::NewPayload { payload, cancun_fields, tx }),
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
                    let (reorg_payload, reorg_cancun_fields) = match create_reorg_head(
                        this.provider,
                        this.evm_config,
                        this.payload_validator,
                        *this.depth,
                        payload.clone(),
                        cancun_fields.clone(),
                    ) {
                        Ok(result) => result,
                        Err(error) => {
                            error!(target: "engine::stream::reorg", %error, "Error attempting to create reorg head");
                            // Forward the payload and attempt to create reorg on top of
                            // the next one
                            return Poll::Ready(Some(BeaconEngineMessage::NewPayload {
                                payload,
                                cancun_fields,
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
                        BeaconEngineMessage::NewPayload { payload, cancun_fields, tx },
                        // Reorg payload
                        BeaconEngineMessage::NewPayload {
                            payload: reorg_payload,
                            cancun_fields: reorg_cancun_fields,
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

fn create_reorg_head<Provider, Evm, Spec>(
    provider: &Provider,
    evm_config: &Evm,
    payload_validator: &ExecutionPayloadValidator<Spec>,
    mut depth: usize,
    next_payload: ExecutionPayload,
    next_cancun_fields: Option<CancunPayloadFields>,
) -> RethResult<(ExecutionPayload, Option<CancunPayloadFields>)>
where
    Provider: BlockReader + StateProviderFactory,
    Evm: ConfigureEvm<Header = Header>,
    Spec: EthereumHardforks,
{
    let chain_spec = payload_validator.chain_spec();

    // Ensure next payload is valid.
    let next_block = payload_validator
        .ensure_well_formed_payload(next_payload, next_cancun_fields.into())
        .map_err(RethError::msg)?;

    // Fetch reorg target block depending on its depth and its parent.
    let mut previous_hash = next_block.parent_hash;
    let mut candidate_transactions = next_block.body.transactions;
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

    // Configure environments
    let mut cfg = CfgEnvWithHandlerCfg::new(Default::default(), Default::default());
    let mut block_env = BlockEnv::default();
    evm_config.fill_cfg_and_block_env(&mut cfg, &mut block_env, &reorg_target.header, U256::MAX);
    let env = EnvWithHandlerCfg::new_with_cfg_env(cfg, block_env, Default::default());
    let mut evm = evm_config.evm_with_env(&mut state, env);

    // apply eip-4788 pre block contract call
    let mut system_caller = SystemCaller::new(evm_config, chain_spec);

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
        let tx_recovered = tx.clone().try_into_ecrecovered().map_err(|_| {
            BlockExecutionError::Validation(BlockValidationError::SenderRecoveryError)
        })?;
        evm_config.fill_tx_env(evm.tx_mut(), &tx_recovered, tx_recovered.signer());
        let exec_result = match evm.transact() {
            Ok(result) => result,
            error @ Err(EVMError::Transaction(_) | EVMError::Header(_)) => {
                trace!(target: "engine::stream::reorg", hash = %tx.hash(), ?error, "Error executing transaction from next block");
                continue
            }
            // Treat error as fatal
            Err(error) => {
                return Err(RethError::Execution(BlockExecutionError::Validation(
                    BlockValidationError::EVM { hash: tx.hash, error: Box::new(error) },
                )))
            }
        };
        evm.db_mut().commit(exec_result.state);

        if let Some(blob_tx) = tx.transaction.as_eip4844() {
            sum_blob_gas_used += blob_tx.blob_gas();
            versioned_hashes.extend(blob_tx.blob_versioned_hashes.clone());
        }

        cumulative_gas_used += exec_result.result.gas_used();
        #[allow(clippy::needless_update)] // side-effect of optimism fields
        receipts.push(Some(Receipt {
            tx_type: tx.tx_type(),
            success: exec_result.result.is_success(),
            cumulative_gas_used,
            logs: exec_result.result.into_logs().into_iter().map(Into::into).collect(),
            ..Default::default()
        }));

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

    let outcome = ExecutionOutcome::new(
        state.take_bundle(),
        Receipts::from(vec![receipts]),
        reorg_target.number,
        Default::default(),
    );
    let hashed_state = HashedPostState::from_bundle_state(&outcome.state().state);

    let (blob_gas_used, excess_blob_gas) =
        if chain_spec.is_cancun_active_at_timestamp(reorg_target.timestamp) {
            (
                Some(sum_blob_gas_used),
                Some(calc_excess_blob_gas(
                    reorg_target_parent.excess_blob_gas.unwrap_or_default(),
                    reorg_target_parent.blob_gas_used.unwrap_or_default(),
                )),
            )
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
            receipts_root: outcome.receipts_root_slow(reorg_target.header.number).unwrap(),
            logs_bloom: outcome.block_logs_bloom(reorg_target.header.number).unwrap(),
            requests_root: None, // TODO(prague)
            gas_used: cumulative_gas_used,
            blob_gas_used: blob_gas_used.map(Into::into),
            excess_blob_gas: excess_blob_gas.map(Into::into),
            state_root: state_provider.state_root(hashed_state)?,
        },
        body: BlockBody {
            transactions,
            ommers: reorg_target.body.ommers,
            withdrawals: reorg_target.body.withdrawals,
            requests: None, // TODO(prague)
        },
    }
    .seal_slow();

    Ok((
        block_to_payload(reorg_block),
        reorg_target
            .header
            .parent_beacon_block_root
            .map(|root| CancunPayloadFields { parent_beacon_block_root: root, versioned_hashes }),
    ))
}

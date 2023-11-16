//! Implementation of parallel executor.

use crate::{
    queue::{TransitionBatch, TransitionQueueStore},
    shared::{SharedState, SharedStateLock},
};
use rayon::prelude::{IntoParallelIterator, ParallelIterator};
use reth_interfaces::{
    executor::{BlockExecutionError, BlockValidationError, ParallelExecutionError},
    RethError, RethResult,
};
use reth_primitives::{
    revm::{
        compat::into_reth_log,
        env::{fill_cfg_and_block_env, fill_tx_env},
    },
    Address, BlockNumber, BlockWithSenders, ChainSpec, Hardfork, PruneModes, PruneSegmentError,
    Receipt, Receipts, TransitionId, TransitionType, U256,
};
use reth_provider::{
    BlockExecutorStats, BlockRangeExecutor, BlockReader, BundleStateWithReceipts, ProviderError,
    PrunableBlockRangeExecutor, TransactionVariant,
};
use reth_revm_executor::{
    processor::verify_receipt, state_change::post_block_balance_increments, ExecutionData,
};
use revm::{
    primitives::{
        Account, AccountStatus, EVMError, Env, ExecutionResult, ResultAndState, State as EVMState,
    },
    DatabaseRef, EVM,
};
use std::{
    collections::{hash_map, BTreeMap, HashMap},
    ops::RangeInclusive,
    sync::Arc,
    time::{Duration, Instant},
};

/// Database boxed with a lifetime and Send.
pub type DatabaseRefBox<'a, E> = Box<dyn DatabaseRef<Error = E> + Send + Sync + 'a>;

pub enum StateTransitionData {
    PreBlock(BlockNumber), // TODO:
    Transaction(BlockNumber, u32, Env),
    PostBlock(BlockNumber, HashMap<Address, u128>),
}

pub enum StateTransition {
    PreBlock(BlockNumber),
    Transaction(BlockNumber, u32, Result<ResultAndState, EVMError<RethError>>),
    PostBlock(BlockNumber, Result<EVMState, EVMError<RethError>>),
}

impl StateTransition {
    fn id(&self) -> TransitionId {
        match self {
            Self::PreBlock(number) => TransitionId(*number, TransitionType::PreBlock),
            Self::Transaction(number, tx_idx, _) => {
                TransitionId(*number, TransitionType::Transaction(*tx_idx))
            }
            Self::PostBlock(number, _) => TransitionId(*number, TransitionType::PostBlock),
        }
    }

    fn block_number(&self) -> BlockNumber {
        match self {
            Self::PreBlock(number) |
            Self::Transaction(number, _, _) |
            Self::PostBlock(number, _) => *number,
        }
    }
}

struct LoadedBlock {
    block: BlockWithSenders,
    td: U256,
    results: Vec<(u32, ExecutionResult)>,
    post_block_executed: bool,
}

impl LoadedBlock {
    fn new(block: BlockWithSenders, td: U256) -> Self {
        let capacity = block.body.len();
        Self { block, td, results: Vec::with_capacity(capacity), post_block_executed: false }
    }

    /// Returns `true` if the block was fully executed.
    fn is_executed(&self) -> bool {
        self.results.len() == self.block.body.len() && self.post_block_executed
    }

    fn env(&self, chain_spec: &ChainSpec) -> Env {
        let mut env = Env::default();
        fill_cfg_and_block_env(
            &mut env.cfg,
            &mut env.block,
            chain_spec,
            &self.block.header,
            self.td,
        );
        env
    }

    fn get_state_transition(
        &self,
        ty: TransitionType,
        chain_spec: &ChainSpec,
    ) -> Option<StateTransitionData> {
        let transition = match ty {
            TransitionType::PreBlock => unimplemented!("pre block transition is not implemented"),
            TransitionType::Transaction(index) => {
                let transaction = self.block.body.get(index as usize)?;
                let sender = self.block.senders.get(index as usize)?;
                let mut env = self.env(chain_spec);
                fill_tx_env(&mut env.tx, transaction, *sender);
                StateTransitionData::Transaction(self.block.number, index, env)
            }
            TransitionType::PostBlock => {
                let increments = post_block_balance_increments(
                    &chain_spec,
                    self.block.number,
                    self.block.difficulty,
                    self.block.beneficiary,
                    self.block.timestamp,
                    self.td,
                    &self.block.ommers,
                    self.block.withdrawals.as_deref(),
                );
                StateTransitionData::PostBlock(self.block.number, increments)
            }
        };
        Some(transition)
    }
}

/// TODO: add docs
#[allow(missing_debug_implementations)]
pub struct ParallelExecutor<'a, Provider> {
    /// Database provider for loading blocks.
    provider: Provider,
    /// EVM state database.
    state: Arc<SharedStateLock<DatabaseRefBox<'a, RethError>>>,
    /// Store for transaction execution order.
    store: Arc<TransitionQueueStore>,
    /// Execution data.
    data: ExecutionData,
    /// Loaded blocks.
    loaded: HashMap<BlockNumber, LoadedBlock>,
    /// Executed blocks pending validation.
    executed: BTreeMap<BlockNumber, LoadedBlock>,
    /// The block number of the next block pending validation.
    next_block_pending_validation: Option<BlockNumber>,
    /// The gas threshold for executing in parallel.
    gas_threshold: u64,
    /// The batch size threshold for executing in parallel.
    batch_size_threshold: u64,
    /// The number of batches executed in parallel.
    batches_executed_in_parallel: u64,
    /// The number of batches executed in sequence.
    batches_executed_in_sequence: u64,
    /// Time spent executing.
    time_executing: Duration,
    /// Time spent executing state transitions.
    time_executing_state_transitions: Duration,
    /// Time spent aggregating state.
    time_aggregating_state: Duration,
    /// Time spent validating.
    time_validating: Duration,
}

impl<'a, Provider: BlockReader> ParallelExecutor<'a, Provider> {
    /// Create new parallel executor.
    pub fn new(
        provider: Provider,
        chain_spec: Arc<ChainSpec>,
        store: Arc<TransitionQueueStore>,
        database: DatabaseRefBox<'a, RethError>,
        gas_threshold: u64,
        batch_size_threshold: u64,
        _num_threads: Option<usize>, // TODO:
    ) -> RethResult<Self> {
        Ok(Self {
            provider,
            store,
            data: ExecutionData::new(chain_spec),
            state: Arc::new(SharedStateLock::new(SharedState::new(database))),
            loaded: HashMap::default(),
            executed: BTreeMap::default(),
            next_block_pending_validation: None,
            gas_threshold,
            batch_size_threshold,
            batches_executed_in_parallel: 0,
            batches_executed_in_sequence: 0,
            time_executing: Duration::from_secs(0),
            time_executing_state_transitions: Duration::from_secs(0),
            time_aggregating_state: Duration::from_secs(0),
            time_validating: Duration::from_secs(0),
        })
    }

    /// Return cloned pointer to the shared state.
    pub fn state(&self) -> Arc<SharedStateLock<DatabaseRefBox<'a, RethError>>> {
        Arc::clone(&self.state)
    }

    fn get_state_transition(
        &mut self,
        transition_id: TransitionId,
    ) -> Result<StateTransitionData, BlockExecutionError> {
        let block_number = transition_id.0;
        let state_transition = match self.loaded.get(&block_number) {
            Some(block) => block.get_state_transition(transition_id.1, &self.data.chain_spec),
            None => {
                tracing::trace!(target: "evm::parallel", block_number, "Loading block");
                let td = self
                    .provider
                    .header_td_by_number(block_number)
                    .unwrap() // TODO:
                    .ok_or_else(|| ProviderError::HeaderNotFound(block_number.into()))?;

                // we need the block's transactions but we don't need the transaction hashes
                let block = self
                    .provider
                    .block_with_senders(block_number, TransactionVariant::NoHash)
                    .unwrap() // TODO:
                    .ok_or_else(|| ProviderError::BlockNotFound(block_number.into()))?;

                let block = LoadedBlock::new(block, td);
                let state_transition =
                    block.get_state_transition(transition_id.1, &self.data.chain_spec);
                self.loaded.insert(block_number, block);
                state_transition
            }
        };
        state_transition.ok_or(BlockExecutionError::Parallel(
            ParallelExecutionError::TransitionNotFound(transition_id),
        ))
    }

    fn execute_state_transition(
        &self,
        transition: StateTransitionData,
    ) -> (StateTransition, Duration) {
        let state = self.state.clone();
        let started_at = Instant::now();
        let transition = match transition {
            StateTransitionData::PreBlock(_) => {
                unimplemented!("pre block transition is not implemented")
            }
            StateTransitionData::Transaction(block_number, idx, env) => {
                let mut evm = EVM::with_env(env);
                evm.database(state);
                StateTransition::Transaction(block_number, idx, evm.transact_ref())
            }
            StateTransitionData::PostBlock(block_number, increments) => {
                // TODO: simplify
                let account_states_result = increments
                    .into_iter()
                    .map(|(address, increment)| {
                        let mut info = state
                            .basic_ref(address)
                            .map_err(EVMError::Database)?
                            .unwrap_or_default();
                        info.balance += U256::from(increment);
                        let status = AccountStatus::Loaded | AccountStatus::Touched;
                        Ok((address, Account { info, status, storage: Default::default() }))
                    })
                    .collect();
                StateTransition::PostBlock(block_number, account_states_result)
            }
        };
        (transition, started_at.elapsed())
    }

    /// Execute a batch of transactions in parallel.
    pub fn execute_batch(&mut self, batch: &TransitionBatch) -> Result<(), BlockExecutionError> {
        let mut transitions = Vec::with_capacity(batch.len());
        for transition_id in batch.iter() {
            transitions.push(self.get_state_transition(*transition_id)?);
        }

        let started_executing_at = Instant::now();
        let gas_per_transition = batch.gas_used / transitions.len() as u128;
        tracing::debug!(target: "evm::parallel", ?batch, "Executing block batch");
        let transition_results: Vec<_> = if transitions.len() > self.batch_size_threshold as usize ||
            gas_per_transition > self.gas_threshold as u128
        {
            self.batches_executed_in_parallel += 1;
            transitions
                .into_par_iter()
                .map(|transition| self.execute_state_transition(transition))
                .collect()
        } else {
            self.batches_executed_in_sequence += 1;
            transitions
                .into_iter()
                .map(|transition| self.execute_state_transition(transition))
                .collect()
        };
        self.time_executing += started_executing_at.elapsed();

        let started_aggregating_state_at = Instant::now();
        let mut states =
            revm::primitives::HashMap::<Address, Vec<(TransitionId, Account)>>::default();
        for (transition_result, duration) in transition_results {
            self.time_executing_state_transitions += duration;
            let transition_id = transition_result.id();
            let mut block = match self.loaded.entry(transition_result.block_number()) {
                hash_map::Entry::Occupied(entry) => entry,
                hash_map::Entry::Vacant(_) => unreachable!("block is pre-loaded"),
            };

            match transition_result {
                StateTransition::PreBlock(_) => {
                    unimplemented!("pre block transition is not implemented")
                }
                StateTransition::Transaction(_, idx, exec_result) => {
                    let ResultAndState { result, state } =
                        exec_result.map_err(|error| ParallelExecutionError::EVM {
                            transition: transition_id,
                            error: error.into(),
                        })?;
                    block.get_mut().results.push((idx, result));
                    for (address, account) in state {
                        states.entry(address).or_default().push((transition_id, account));
                    }
                }
                StateTransition::PostBlock(_, increment_result) => {
                    let increments =
                        increment_result.map_err(|error| ParallelExecutionError::EVM {
                            transition: transition_id,
                            error: error.into(),
                        })?;
                    block.get_mut().post_block_executed = true;
                    for (address, account) in increments {
                        states.entry(address).or_default().push((transition_id, account));
                    }
                }
            };

            if block.get().is_executed() {
                let executed = block.remove();
                let block_number = executed.block.number;
                tracing::trace!(target: "evm::parallel", block_number, "Block has been fully executed");
                self.executed.insert(block_number, executed);
            }
        }

        tracing::trace!(target: "evm::parallel", "Committing transition batch");
        self.state.write().commit(states);
        self.time_aggregating_state += started_aggregating_state_at.elapsed();

        Ok(())
    }

    fn execute_range_inner(
        &mut self,
        range: RangeInclusive<BlockNumber>,
        should_verify_receipts: bool,
    ) -> Result<(), BlockExecutionError> {
        let queue = self.store.load(range.clone()).unwrap().unwrap();

        if self.data.first_block.is_none() {
            self.data.first_block = Some(*range.start());
        }
        self.next_block_pending_validation = Some(*range.start());

        // TODO:
        // Set state clear flag.
        // let state_clear_enabled = self.data.state_clear_enabled(block.number);
        // self.state.write().set_state_clear_flag(state_clear_enabled);

        for batch in queue.batches().iter().filter(|b| !b.is_empty()) {
            self.execute_batch(batch)?;

            let started_validation_at = Instant::now();
            'validation: while self.next_block_pending_validation.as_ref() ==
                self.executed.keys().next()
            {
                let (_, mut executed) = self.executed.pop_first().unwrap();
                executed.results.sort_unstable_by_key(|(idx, _)| *idx);

                let block_number = executed.block.number;
                tracing::trace!(target: "evm::parallel", block_number, "Validating executed block");

                let retention = self.data.retention_for_block(executed.block.number);
                self.state.write().merge_transitions(executed.block.number, retention);

                let mut cumulative_gas_used = 0;
                let mut receipts = Vec::with_capacity(executed.block.body.len());
                for (transaction, (_, result)) in executed.block.body.iter().zip(executed.results) {
                    cumulative_gas_used += result.gas_used();
                    receipts.push(Receipt {
                        tx_type: transaction.tx_type(),
                        // Success flag was added in `EIP-658: Embedding transaction status code in
                        // receipts`.
                        success: result.is_success(),
                        cumulative_gas_used,
                        // convert to reth log
                        logs: result.into_logs().into_iter().map(into_reth_log).collect(),
                    });
                }

                // Check if gas used matches the value set in header.
                if executed.block.gas_used != cumulative_gas_used {
                    let receipts = Receipts::from_block_receipt(receipts);
                    return Err(BlockValidationError::BlockGasUsed {
                        got: cumulative_gas_used,
                        expected: executed.block.gas_used,
                        gas_spent_by_tx: receipts.gas_spent_by_tx()?,
                    }
                    .into())
                }

                // TODO Before Byzantium, receipts contained state root that would mean that
                // expensive operation as hashing that is needed for state root got
                // calculated in every transaction This was replaced with is_success
                // flag. See more about EIP here: https://eips.ethereum.org/EIPS/eip-658
                if should_verify_receipts &&
                    self.data
                        .chain_spec
                        .fork(Hardfork::Byzantium)
                        .active_at_block(executed.block.number)
                {
                    if let Err(error) = verify_receipt(
                        executed.block.receipts_root,
                        executed.block.logs_bloom,
                        receipts.iter(),
                    ) {
                        tracing::debug!(target: "evm::parallel", ?error, ?receipts, "receipts verification failed");
                        return Err(error.into())
                    };
                }

                self.save_receipts(receipts)?;

                if executed.block.number == *range.end() {
                    self.next_block_pending_validation = None;
                    break 'validation
                }

                self.next_block_pending_validation = Some(executed.block.number + 1);
            }
            self.time_validating += started_validation_at.elapsed();
        }

        if let Some(unvalidated_block) = self.next_block_pending_validation {
            return Err(
                ParallelExecutionError::InconsistentTransitionQueue { unvalidated_block }.into()
            )
        }

        tracing::info!(
            target: "evm::parallel",
            ?range,
            parallel = self.batches_executed_in_parallel,
            sequential = self.batches_executed_in_sequence,
            time_executing_ms = self.time_executing.as_millis(),
            time_executing_state_transitions_ms = self.time_executing_state_transitions.as_millis(),
            time_aggregating_state_ms = self.time_aggregating_state.as_millis(),
            time_validating_ms = self.time_validating.as_millis(),
            "Executed batches in range"
        );
        self.batches_executed_in_parallel = 0;
        self.batches_executed_in_sequence = 0;
        self.time_executing = Duration::from_secs(0);
        self.time_aggregating_state = Duration::from_secs(0);
        self.time_validating = Duration::from_secs(0);

        Ok(())
    }

    /// Saves receipts to the executor.
    pub fn save_receipts(&mut self, receipts: Vec<Receipt>) -> Result<(), PruneSegmentError> {
        let mut receipts = receipts.into_iter().map(Option::Some).collect();
        // Prune receipts if necessary.
        self.data.prune_receipts(&mut receipts)?;
        // Save receipts.
        self.data.receipts.push(receipts);
        Ok(())
    }
}

impl<Provider> BlockRangeExecutor for ParallelExecutor<'_, Provider>
where
    Provider: BlockReader,
{
    fn execute_range(
        &mut self,
        range: RangeInclusive<BlockNumber>,
        should_verify_receipts: bool,
    ) -> Result<(), BlockExecutionError> {
        self.execute_range_inner(range, should_verify_receipts)
    }

    fn take_output_state(&mut self) -> BundleStateWithReceipts {
        let bundle_state = self.state.write().take_bundle();
        let receipts = std::mem::take(&mut self.data.receipts);
        BundleStateWithReceipts::new(
            bundle_state,
            receipts,
            self.data.first_block.unwrap_or_default(),
        )
    }

    fn stats(&self) -> BlockExecutorStats {
        // TODO:
        BlockExecutorStats::default()
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.state.read().bundle_size_hint())
    }
}

impl<Provider> PrunableBlockRangeExecutor for ParallelExecutor<'_, Provider>
where
    Provider: BlockReader,
{
    fn set_tip(&mut self, tip: BlockNumber) {
        self.data.tip = Some(tip);
    }

    fn set_prune_modes(&mut self, prune_modes: PruneModes) {
        self.data.prune_modes = prune_modes;
    }
}

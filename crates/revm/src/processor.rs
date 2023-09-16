use crate::{
    database::StateProviderDatabase,
    env::{fill_cfg_and_block_env, fill_tx_env},
    eth_dao_fork::{DAO_HARDFORK_BENEFICIARY, DAO_HARDKFORK_ACCOUNTS},
    into_reth_log,
    stack::{InspectorStack, InspectorStackConfig},
    state_change::post_block_balance_increments,
};
use reth_interfaces::{
    executor::{BlockExecutionError, BlockValidationError},
    Error,
};
use reth_primitives::{
    Address, Block, BlockNumber, Bloom, ChainSpec, Hardfork, Header, PruneMode, PruneModes,
    PrunePartError, Receipt, ReceiptWithBloom, TransactionSigned, H256, MINIMUM_PRUNING_DISTANCE,
    U256,
};
use reth_provider::{
    BlockExecutor, BlockExecutorStats, BundleStateWithReceipts, PrunableBlockExecutor,
    StateProvider,
};
use revm::{
    db::{states::bundle_state::BundleRetention, StateDBBox},
    primitives::ResultAndState,
    DatabaseCommit, State, EVM,
};
use std::{sync::Arc, time::Instant};
use tracing::{debug, trace};

/// EVMProcessor is a block executor that uses revm to execute blocks or multiple blocks.
///
/// Output is obtained by calling `take_output_state` function.
///
/// It is capable of pruning the data that will be written to the database
/// and implemented [PrunableBlockExecutor] traits.
///
/// It implemented the [BlockExecutor] that give it the ability to take block
/// apply pre state (Cancun system contract call), execute transaction and apply
/// state change and then apply post execution changes (block reward, withdrawals, irregular DAO
/// hardfork state change). And if `execute_and_verify_receipt` is called it will verify the
/// receipt.
///
/// InspectorStack are used for optional inspecting execution. And it contains
/// various duration of parts of execution.
pub struct EVMProcessor<'a> {
    /// The configured chain-spec
    chain_spec: Arc<ChainSpec>,
    /// revm instance that contains database and env environment.
    evm: EVM<StateDBBox<'a, Error>>,
    /// Hook and inspector stack that we want to invoke on that hook.
    stack: InspectorStack,
    /// The collection of receipts.
    /// Outer vector stores receipts for each block sequentially.
    /// The inner vector stores receipts ordered by transaction number.
    ///
    /// If receipt is None it means it is pruned.   
    receipts: Vec<Vec<Option<Receipt>>>,
    /// First block will be initialized to `None`
    /// and be set to the block number of first block executed.
    first_block: Option<BlockNumber>,
    /// The maximum known block.
    tip: Option<BlockNumber>,
    /// Pruning configuration.
    prune_modes: PruneModes,
    /// Memoized address pruning filter.
    /// Empty implies that there is going to be addresses to include in the filter in a future
    /// block. None means there isn't any kind of configuration.
    pruning_address_filter: Option<(u64, Vec<Address>)>,
    /// Execution stats
    stats: BlockExecutorStats,
}

impl<'a> EVMProcessor<'a> {
    /// Return chain spec.
    pub fn chain_spec(&self) -> &Arc<ChainSpec> {
        &self.chain_spec
    }

    /// Create a new pocessor with the given chain spec.
    pub fn new(chain_spec: Arc<ChainSpec>) -> Self {
        let evm = EVM::new();
        EVMProcessor {
            chain_spec,
            evm,
            stack: InspectorStack::new(InspectorStackConfig::default()),
            receipts: Vec::new(),
            first_block: None,
            tip: None,
            prune_modes: PruneModes::none(),
            pruning_address_filter: None,
            stats: BlockExecutorStats::default(),
        }
    }

    /// Creates a new executor from the given chain spec and database.
    pub fn new_with_db<DB: StateProvider + 'a>(
        chain_spec: Arc<ChainSpec>,
        db: StateProviderDatabase<DB>,
    ) -> Self {
        let state = State::builder()
            .with_database_boxed(Box::new(db))
            .with_bundle_update()
            .without_state_clear()
            .build();
        EVMProcessor::new_with_state(chain_spec, state)
    }

    /// Create a new EVM processor with the given revm state.
    pub fn new_with_state(chain_spec: Arc<ChainSpec>, revm_state: StateDBBox<'a, Error>) -> Self {
        let mut evm = EVM::new();
        evm.database(revm_state);
        EVMProcessor {
            chain_spec,
            evm,
            stack: InspectorStack::new(InspectorStackConfig::default()),
            receipts: Vec::new(),
            first_block: None,
            tip: None,
            prune_modes: PruneModes::none(),
            pruning_address_filter: None,
            stats: BlockExecutorStats::default(),
        }
    }

    /// Configures the executor with the given inspectors.
    pub fn set_stack(&mut self, stack: InspectorStack) {
        self.stack = stack;
    }

    /// Returns a reference to the database
    pub fn db_mut(&mut self) -> &mut StateDBBox<'a, Error> {
        // Option will be removed from EVM in the future.
        // as it is always some.
        // https://github.com/bluealloy/revm/issues/697
        self.evm.db().expect("Database inside EVM is always set")
    }

    fn recover_senders(
        &mut self,
        body: &[TransactionSigned],
        senders: Option<Vec<Address>>,
    ) -> Result<Vec<Address>, BlockExecutionError> {
        if let Some(senders) = senders {
            if body.len() == senders.len() {
                Ok(senders)
            } else {
                Err(BlockValidationError::SenderRecoveryError.into())
            }
        } else {
            let time = Instant::now();
            let ret = TransactionSigned::recover_signers(body, body.len())
                .ok_or(BlockValidationError::SenderRecoveryError.into());
            self.stats.sender_recovery_duration += time.elapsed();
            ret
        }
    }

    /// Initializes the config and block env.
    fn init_env(&mut self, header: &Header, total_difficulty: U256) {
        // Set state clear flag.
        let state_clear_flag =
            self.chain_spec.fork(Hardfork::SpuriousDragon).active_at_block(header.number);

        self.db_mut().set_state_clear_flag(state_clear_flag);

        fill_cfg_and_block_env(
            &mut self.evm.env.cfg,
            &mut self.evm.env.block,
            &self.chain_spec,
            header,
            total_difficulty,
        );
    }

    /// Apply post execution state changes, including block rewards, withdrawals, and irregular DAO
    /// hardfork state change.
    pub fn apply_post_execution_state_change(
        &mut self,
        block: &Block,
        total_difficulty: U256,
    ) -> Result<(), BlockExecutionError> {
        let mut balance_increments = post_block_balance_increments(
            &self.chain_spec,
            block.number,
            block.difficulty,
            block.beneficiary,
            block.timestamp,
            total_difficulty,
            &block.ommers,
            block.withdrawals.as_deref(),
        );

        // Irregular state change at Ethereum DAO hardfork
        if self.chain_spec.fork(Hardfork::Dao).transitions_at_block(block.number) {
            // drain balances from hardcoded addresses.
            let drained_balance: u128 = self
                .db_mut()
                .drain_balances(DAO_HARDKFORK_ACCOUNTS)
                .map_err(|_| BlockValidationError::IncrementBalanceFailed)?
                .into_iter()
                .sum();

            // return balance to DAO beneficiary.
            *balance_increments.entry(DAO_HARDFORK_BENEFICIARY).or_default() += drained_balance;
        }
        // increment balances
        self.db_mut()
            .increment_balances(balance_increments.into_iter().map(|(k, v)| (k, v)))
            .map_err(|_| BlockValidationError::IncrementBalanceFailed)?;

        Ok(())
    }

    /// Runs a single transaction in the configured environment and proceeds
    /// to return the result and state diff (without applying it).
    ///
    /// Assumes the rest of the block environment has been filled via `init_block_env`.
    pub fn transact(
        &mut self,
        transaction: &TransactionSigned,
        sender: Address,
    ) -> Result<ResultAndState, BlockExecutionError> {
        // Fill revm structure.
        fill_tx_env(&mut self.evm.env.tx, transaction, sender);

        let hash = transaction.hash();
        let out = if self.stack.should_inspect(&self.evm.env, hash) {
            // execution with inspector.
            let output = self.evm.inspect(&mut self.stack);
            tracing::trace!(
                target: "evm",
                ?hash, ?output, ?transaction, env = ?self.evm.env,
                "Executed transaction"
            );
            output
        } else {
            // main execution.
            self.evm.transact()
        };
        out.map_err(|e| BlockValidationError::EVM { hash, message: format!("{e:?}") }.into())
    }

    /// Runs the provided transactions and commits their state to the run-time database.
    ///
    /// The returned [BundleStateWithReceipts] can be used to persist the changes to disk, and
    /// contains the changes made by each transaction.
    ///
    /// The changes in [BundleStateWithReceipts] have a transition ID associated with them: there is
    /// one transition ID for each transaction (with the first executed tx having transition ID
    /// 0, and so on).
    ///
    /// The second returned value represents the total gas used by this block of transactions.
    pub fn execute_transactions(
        &mut self,
        block: &Block,
        total_difficulty: U256,
        senders: Option<Vec<Address>>,
    ) -> Result<(Vec<Receipt>, u64), BlockExecutionError> {
        // perf: do not execute empty blocks
        if block.body.is_empty() {
            return Ok((Vec::new(), 0))
        }

        let senders = self.recover_senders(&block.body, senders)?;

        self.init_env(&block.header, total_difficulty);

        let mut cumulative_gas_used = 0;
        let mut receipts = Vec::with_capacity(block.body.len());
        for (transaction, sender) in block.body.iter().zip(senders) {
            let time = Instant::now();
            // The sum of the transaction’s gas limit, Tg, and the gas utilized in this block prior,
            // must be no greater than the block’s gasLimit.
            let block_available_gas = block.header.gas_limit - cumulative_gas_used;
            if transaction.gas_limit() > block_available_gas {
                return Err(BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                    transaction_gas_limit: transaction.gas_limit(),
                    block_available_gas,
                }
                .into())
            }
            // Execute transaction.
            let ResultAndState { result, state } = self.transact(transaction, sender)?;
            trace!(
                target: "evm",
                ?transaction, ?result, ?state,
                "Executed transaction"
            );
            self.stats.execution_duration += time.elapsed();
            let time = Instant::now();

            self.db_mut().commit(state);

            self.stats.apply_state_duration += time.elapsed();

            // append gas used
            cumulative_gas_used += result.gas_used();

            // Push transaction changeset and calculate header bloom filter for receipt.
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

        Ok((receipts, cumulative_gas_used))
    }

    /// Execute the block, verify gas usage and apply post-block state changes.
    fn execute_inner(
        &mut self,
        block: &Block,
        total_difficulty: U256,
        senders: Option<Vec<Address>>,
    ) -> Result<Vec<Receipt>, BlockExecutionError> {
        let (receipts, cumulative_gas_used) =
            self.execute_transactions(block, total_difficulty, senders)?;

        // Check if gas used matches the value set in header.
        if block.gas_used != cumulative_gas_used {
            return Err(BlockValidationError::BlockGasUsed {
                got: cumulative_gas_used,
                expected: block.gas_used,
                gas_spent_by_tx: self
                    .receipts
                    .last()
                    .map(|block_r| {
                        block_r
                            .iter()
                            .enumerate()
                            .map(|(id, tx_r)| {
                                (
                                    id as u64,
                                    tx_r.as_ref()
                                        .expect("receipts have not been pruned")
                                        .cumulative_gas_used,
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            }
            .into())
        }
        let time = Instant::now();
        self.apply_post_execution_state_change(block, total_difficulty)?;
        self.stats.apply_post_execution_state_changes_duration += time.elapsed();

        let time = Instant::now();
        let retention = if self.tip.map_or(true, |tip| {
            !self.prune_modes.should_prune_account_history(block.number, tip) &&
                !self.prune_modes.should_prune_storage_history(block.number, tip)
        }) {
            BundleRetention::Reverts
        } else {
            BundleRetention::PlainState
        };
        self.db_mut().merge_transitions(retention);
        self.stats.merge_transitions_duration += time.elapsed();

        if self.first_block.is_none() {
            self.first_block = Some(block.number);
        }

        Ok(receipts)
    }

    /// Save receipts to the executor.
    pub fn save_receipts(&mut self, receipts: Vec<Receipt>) -> Result<(), BlockExecutionError> {
        let mut receipts = receipts.into_iter().map(Option::Some).collect();
        // Prune receipts if necessary.
        self.prune_receipts(&mut receipts)?;
        // Save receipts.
        self.receipts.push(receipts);
        Ok(())
    }

    /// Prune receipts according to the pruning configuration.
    fn prune_receipts(
        &mut self,
        receipts: &mut Vec<Option<Receipt>>,
    ) -> Result<(), PrunePartError> {
        let (first_block, tip) = match self.first_block.zip(self.tip) {
            Some((block, tip)) => (block, tip),
            _ => return Ok(()),
        };

        let block_number = first_block + self.receipts.len() as u64;

        // Block receipts should not be retained
        if self.prune_modes.receipts == Some(PruneMode::Full) ||
                // [`PrunePart::Receipts`] takes priority over [`PrunePart::ContractLogs`]
                self.prune_modes.should_prune_receipts(block_number, tip)
        {
            receipts.clear();
            return Ok(())
        }

        // All receipts from the last 128 blocks are required for blockchain tree, even with
        // [`PrunePart::ContractLogs`].
        let prunable_receipts =
            PruneMode::Distance(MINIMUM_PRUNING_DISTANCE).should_prune(block_number, tip);
        if !prunable_receipts {
            return Ok(())
        }

        let contract_log_pruner = self.prune_modes.receipts_log_filter.group_by_block(tip, None)?;

        if !contract_log_pruner.is_empty() {
            let (prev_block, filter) = self.pruning_address_filter.get_or_insert((0, Vec::new()));
            for (_, addresses) in contract_log_pruner.range(*prev_block..=block_number) {
                filter.extend(addresses.iter().copied());
            }
        }

        for receipt in receipts.iter_mut() {
            let inner_receipt = receipt.as_ref().expect("receipts have not been pruned");

            // If there is an address_filter, and it does not contain any of the
            // contract addresses, then remove this receipts
            if let Some((_, filter)) = &self.pruning_address_filter {
                if !inner_receipt.logs.iter().any(|log| filter.contains(&log.address)) {
                    receipt.take();
                }
            }
        }

        Ok(())
    }
}

impl<'a> BlockExecutor for EVMProcessor<'a> {
    fn execute(
        &mut self,
        block: &Block,
        total_difficulty: U256,
        senders: Option<Vec<Address>>,
    ) -> Result<(), BlockExecutionError> {
        let receipts = self.execute_inner(block, total_difficulty, senders)?;
        self.save_receipts(receipts)
    }

    fn execute_and_verify_receipt(
        &mut self,
        block: &Block,
        total_difficulty: U256,
        senders: Option<Vec<Address>>,
    ) -> Result<(), BlockExecutionError> {
        // execute block
        let receipts = self.execute_inner(block, total_difficulty, senders)?;

        // TODO Before Byzantium, receipts contained state root that would mean that expensive
        // operation as hashing that is needed for state root got calculated in every
        // transaction This was replaced with is_success flag.
        // See more about EIP here: https://eips.ethereum.org/EIPS/eip-658
        if self.chain_spec.fork(Hardfork::Byzantium).active_at_block(block.header.number) {
            let time = Instant::now();
            if let Err(error) =
                verify_receipt(block.header.receipts_root, block.header.logs_bloom, receipts.iter())
            {
                debug!(target: "evm", ?error, ?receipts, "receipts verification failed");
                return Err(error)
            };
            self.stats.receipt_root_duration += time.elapsed();
        }

        self.save_receipts(receipts)
    }

    fn take_output_state(&mut self) -> BundleStateWithReceipts {
        let receipts = std::mem::take(&mut self.receipts);
        BundleStateWithReceipts::new(
            self.evm.db().unwrap().take_bundle(),
            receipts,
            self.first_block.unwrap_or_default(),
        )
    }

    fn stats(&self) -> BlockExecutorStats {
        self.stats.clone()
    }

    fn size_hint(&self) -> Option<usize> {
        self.evm.db.as_ref().map(|db| db.bundle_size_hint())
    }
}

impl<'a> PrunableBlockExecutor for EVMProcessor<'a> {
    fn set_tip(&mut self, tip: BlockNumber) {
        self.tip = Some(tip);
    }

    fn set_prune_modes(&mut self, prune_modes: PruneModes) {
        self.prune_modes = prune_modes;
    }
}

/// Verify receipts
pub fn verify_receipt<'a>(
    expected_receipts_root: H256,
    expected_logs_bloom: Bloom,
    receipts: impl Iterator<Item = &'a Receipt> + Clone,
) -> Result<(), BlockExecutionError> {
    // Check receipts root.
    let receipts_with_bloom = receipts.map(|r| r.clone().into()).collect::<Vec<ReceiptWithBloom>>();
    let receipts_root = reth_primitives::proofs::calculate_receipt_root(&receipts_with_bloom);
    if receipts_root != expected_receipts_root {
        return Err(BlockValidationError::ReceiptRootDiff {
            got: receipts_root,
            expected: expected_receipts_root,
        }
        .into())
    }

    // Create header log bloom.
    let logs_bloom = receipts_with_bloom.iter().fold(Bloom::zero(), |bloom, r| bloom | r.bloom);
    if logs_bloom != expected_logs_bloom {
        return Err(BlockValidationError::BloomLogDiff {
            expected: Box::new(expected_logs_bloom),
            got: Box::new(logs_bloom),
        }
        .into())
    }

    Ok(())
}

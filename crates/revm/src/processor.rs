use crate::{
    database::StateProviderDatabase,
    eth_dao_fork::{DAO_HARDFORK_BENEFICIARY, DAO_HARDKFORK_ACCOUNTS},
    stack::{InspectorStack, InspectorStackConfig},
    state_change::{apply_beacon_root_contract_call, post_block_balance_increments},
};
use reth_interfaces::executor::{BlockExecutionError, BlockValidationError};
use reth_primitives::{
    revm::env::{fill_cfg_and_block_env, fill_tx_env},
    Address, Block, BlockNumber, BlockWithSenders, Bloom, ChainSpec, GotExpected, Hardfork, Header,
    PruneMode, PruneModes, PruneSegmentError, Receipt, ReceiptWithBloom, Receipts,
    TransactionSigned, B256, MINIMUM_PRUNING_DISTANCE, U256,
};
use reth_provider::{
    BlockExecutor, BlockExecutorStats, ProviderError, PrunableBlockExecutor, StateProvider,
};
use revm::{
    db::{states::bundle_state::BundleRetention, StateDBBox},
    primitives::ResultAndState,
    State, EVM,
};
use std::{sync::Arc, time::Instant};

#[cfg(not(feature = "optimism"))]
use reth_primitives::revm::compat::into_reth_log;
#[cfg(not(feature = "optimism"))]
use reth_provider::BundleStateWithReceipts;
#[cfg(not(feature = "optimism"))]
use revm::DatabaseCommit;
#[cfg(not(feature = "optimism"))]
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
// TODO: https://github.com/bluealloy/revm/pull/745
// #[derive(Debug)]
#[allow(missing_debug_implementations)]
pub struct EVMProcessor<'a> {
    /// The configured chain-spec
    pub(crate) chain_spec: Arc<ChainSpec>,
    /// revm instance that contains database and env environment.
    pub(crate) evm: EVM<StateDBBox<'a, ProviderError>>,
    /// Hook and inspector stack that we want to invoke on that hook.
    stack: InspectorStack,
    /// The collection of receipts.
    /// Outer vector stores receipts for each block sequentially.
    /// The inner vector stores receipts ordered by transaction number.
    ///
    /// If receipt is None it means it is pruned.
    pub(crate) receipts: Receipts,
    /// First block will be initialized to `None`
    /// and be set to the block number of first block executed.
    pub(crate) first_block: Option<BlockNumber>,
    /// The maximum known block.
    tip: Option<BlockNumber>,
    /// Pruning configuration.
    prune_modes: PruneModes,
    /// Memoized address pruning filter.
    /// Empty implies that there is going to be addresses to include in the filter in a future
    /// block. None means there isn't any kind of configuration.
    pruning_address_filter: Option<(u64, Vec<Address>)>,
    /// Execution stats
    pub(crate) stats: BlockExecutorStats,
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
            receipts: Receipts::new(),
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
    pub fn new_with_state(
        chain_spec: Arc<ChainSpec>,
        revm_state: StateDBBox<'a, ProviderError>,
    ) -> Self {
        let mut evm = EVM::new();
        evm.database(revm_state);
        EVMProcessor {
            chain_spec,
            evm,
            stack: InspectorStack::new(InspectorStackConfig::default()),
            receipts: Receipts::new(),
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

    /// Configure the executor with the given block.
    pub fn set_first_block(&mut self, num: BlockNumber) {
        self.first_block = Some(num);
    }

    /// Returns a reference to the database
    pub fn db_mut(&mut self) -> &mut StateDBBox<'a, ProviderError> {
        // Option will be removed from EVM in the future.
        // as it is always some.
        // https://github.com/bluealloy/revm/issues/697
        self.evm.db().expect("Database inside EVM is always set")
    }

    /// Initializes the config and block env.
    pub(crate) fn init_env(&mut self, header: &Header, total_difficulty: U256) {
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

    /// Applies the pre-block call to the EIP-4788 beacon block root contract.
    ///
    /// If cancun is not activated or the block is the genesis block, then this is a no-op, and no
    /// state changes are made.
    pub fn apply_beacon_root_contract_call(
        &mut self,
        block: &Block,
    ) -> Result<(), BlockExecutionError> {
        apply_beacon_root_contract_call(
            &self.chain_spec,
            block.timestamp,
            block.number,
            block.parent_beacon_block_root,
            &mut self.evm,
        )?;
        Ok(())
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
            .increment_balances(balance_increments)
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
        #[cfg(not(feature = "optimism"))]
        fill_tx_env(&mut self.evm.env.tx, transaction, sender);

        #[cfg(feature = "optimism")]
        {
            let mut envelope_buf = Vec::with_capacity(transaction.length_without_header());
            transaction.encode_enveloped(&mut envelope_buf);
            fill_tx_env(&mut self.evm.env.tx, transaction, sender, envelope_buf.into());
        }

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
        out.map_err(|e| BlockValidationError::EVM { hash, error: e.into() }.into())
    }

    /// Execute the block, verify gas usage and apply post-block state changes.
    pub(crate) fn execute_inner(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<Vec<Receipt>, BlockExecutionError> {
        self.init_env(&block.header, total_difficulty);
        self.apply_beacon_root_contract_call(block)?;
        let (receipts, cumulative_gas_used) = self.execute_transactions(block, total_difficulty)?;

        // Check if gas used matches the value set in header.
        if block.gas_used != cumulative_gas_used {
            let receipts = Receipts::from_block_receipt(receipts);
            return Err(BlockValidationError::BlockGasUsed {
                gas: GotExpected { got: cumulative_gas_used, expected: block.gas_used },
                gas_spent_by_tx: receipts.gas_spent_by_tx()?,
            }
            .into())
        }
        let time = Instant::now();
        self.apply_post_execution_state_change(block, total_difficulty)?;
        self.stats.apply_post_execution_state_changes_duration += time.elapsed();

        let time = Instant::now();
        let retention = if self.tip.map_or(true, |tip| {
            !self
                .prune_modes
                .account_history
                .map_or(false, |mode| mode.should_prune(block.number, tip)) &&
                !self
                    .prune_modes
                    .storage_history
                    .map_or(false, |mode| mode.should_prune(block.number, tip))
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
    ) -> Result<(), PruneSegmentError> {
        let (first_block, tip) = match self.first_block.zip(self.tip) {
            Some((block, tip)) => (block, tip),
            _ => return Ok(()),
        };

        let block_number = first_block + self.receipts.len() as u64;

        // Block receipts should not be retained
        if self.prune_modes.receipts == Some(PruneMode::Full) ||
                // [`PruneSegment::Receipts`] takes priority over [`PruneSegment::ContractLogs`]
            self.prune_modes.receipts.map_or(false, |mode| mode.should_prune(block_number, tip))
        {
            receipts.clear();
            return Ok(())
        }

        // All receipts from the last 128 blocks are required for blockchain tree, even with
        // [`PruneSegment::ContractLogs`].
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

/// Default Ethereum implementation of the [BlockExecutor] trait for the [EVMProcessor].
#[cfg(not(feature = "optimism"))]
impl<'a> BlockExecutor for EVMProcessor<'a> {
    fn execute(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<(), BlockExecutionError> {
        let receipts = self.execute_inner(block, total_difficulty)?;
        self.save_receipts(receipts)
    }

    fn execute_and_verify_receipt(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<(), BlockExecutionError> {
        // execute block
        let receipts = self.execute_inner(block, total_difficulty)?;

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

    fn execute_transactions(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<(Vec<Receipt>, u64), BlockExecutionError> {
        self.init_env(&block.header, total_difficulty);

        // perf: do not execute empty blocks
        if block.body.is_empty() {
            return Ok((Vec::new(), 0))
        }

        let mut cumulative_gas_used = 0;
        let mut receipts = Vec::with_capacity(block.body.len());
        for (sender, transaction) in block.transactions_with_sender() {
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
            let ResultAndState { result, state } = self.transact(transaction, *sender)?;
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
    expected_receipts_root: B256,
    expected_logs_bloom: Bloom,
    receipts: impl Iterator<Item = &'a Receipt> + Clone,
    #[cfg(feature = "optimism")] chain_spec: &ChainSpec,
    #[cfg(feature = "optimism")] timestamp: u64,
) -> Result<(), BlockExecutionError> {
    // Check receipts root.
    let receipts_with_bloom = receipts.map(|r| r.clone().into()).collect::<Vec<ReceiptWithBloom>>();
    let receipts_root = reth_primitives::proofs::calculate_receipt_root(
        &receipts_with_bloom,
        #[cfg(feature = "optimism")]
        chain_spec,
        #[cfg(feature = "optimism")]
        timestamp,
    );
    if receipts_root != expected_receipts_root {
        return Err(BlockValidationError::ReceiptRootDiff(
            GotExpected { got: receipts_root, expected: expected_receipts_root }.into(),
        )
        .into())
    }

    // Create header log bloom.
    let logs_bloom = receipts_with_bloom.iter().fold(Bloom::ZERO, |bloom, r| bloom | r.bloom);
    if logs_bloom != expected_logs_bloom {
        return Err(BlockValidationError::BloomLogDiff(
            GotExpected { got: logs_bloom, expected: expected_logs_bloom }.into(),
        )
        .into())
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_interfaces::provider::ProviderResult;
    use reth_primitives::{
        bytes,
        constants::{BEACON_ROOTS_ADDRESS, SYSTEM_ADDRESS},
        keccak256,
        trie::AccountProof,
        Account, Bytecode, Bytes, ChainSpecBuilder, ForkCondition, StorageKey, MAINNET,
    };
    use reth_provider::{
        AccountReader, BlockHashReader, BundleStateWithReceipts, StateRootProvider,
    };
    use reth_trie::updates::TrieUpdates;
    use revm::{Database, TransitionState};
    use std::collections::HashMap;

    static BEACON_ROOT_CONTRACT_CODE: Bytes = bytes!("3373fffffffffffffffffffffffffffffffffffffffe14604d57602036146024575f5ffd5b5f35801560495762001fff810690815414603c575f5ffd5b62001fff01545f5260205ff35b5f5ffd5b62001fff42064281555f359062001fff015500");

    #[derive(Debug, Default, Clone, Eq, PartialEq)]
    struct StateProviderTest {
        accounts: HashMap<Address, (HashMap<StorageKey, U256>, Account)>,
        contracts: HashMap<B256, Bytecode>,
        block_hash: HashMap<u64, B256>,
    }

    impl StateProviderTest {
        /// Insert account.
        fn insert_account(
            &mut self,
            address: Address,
            mut account: Account,
            bytecode: Option<Bytes>,
            storage: HashMap<StorageKey, U256>,
        ) {
            if let Some(bytecode) = bytecode {
                let hash = keccak256(&bytecode);
                account.bytecode_hash = Some(hash);
                self.contracts.insert(hash, Bytecode::new_raw(bytecode));
            }
            self.accounts.insert(address, (storage, account));
        }
    }

    impl AccountReader for StateProviderTest {
        fn basic_account(&self, address: Address) -> ProviderResult<Option<Account>> {
            Ok(self.accounts.get(&address).map(|(_, acc)| *acc))
        }
    }

    impl BlockHashReader for StateProviderTest {
        fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
            Ok(self.block_hash.get(&number).cloned())
        }

        fn canonical_hashes_range(
            &self,
            start: BlockNumber,
            end: BlockNumber,
        ) -> ProviderResult<Vec<B256>> {
            let range = start..end;
            Ok(self
                .block_hash
                .iter()
                .filter_map(|(block, hash)| range.contains(block).then_some(*hash))
                .collect())
        }
    }

    impl StateRootProvider for StateProviderTest {
        fn state_root(&self, _bundle_state: &BundleStateWithReceipts) -> ProviderResult<B256> {
            unimplemented!("state root computation is not supported")
        }

        fn state_root_with_updates(
            &self,
            _bundle_state: &BundleStateWithReceipts,
        ) -> ProviderResult<(B256, TrieUpdates)> {
            unimplemented!("state root computation is not supported")
        }
    }

    impl StateProvider for StateProviderTest {
        fn storage(
            &self,
            account: Address,
            storage_key: StorageKey,
        ) -> ProviderResult<Option<reth_primitives::StorageValue>> {
            Ok(self
                .accounts
                .get(&account)
                .and_then(|(storage, _)| storage.get(&storage_key).cloned()))
        }

        fn bytecode_by_hash(&self, code_hash: B256) -> ProviderResult<Option<Bytecode>> {
            Ok(self.contracts.get(&code_hash).cloned())
        }

        fn proof(&self, _address: Address, _keys: &[B256]) -> ProviderResult<AccountProof> {
            unimplemented!("proof generation is not supported")
        }
    }

    #[test]
    fn eip_4788_non_genesis_call() {
        let mut header =
            Header { timestamp: 1, number: 1, excess_blob_gas: Some(0), ..Header::default() };

        let mut db = StateProviderTest::default();

        let beacon_root_contract_account = Account {
            balance: U256::ZERO,
            bytecode_hash: Some(keccak256(BEACON_ROOT_CONTRACT_CODE.clone())),
            nonce: 1,
        };

        db.insert_account(
            BEACON_ROOTS_ADDRESS,
            beacon_root_contract_account,
            Some(BEACON_ROOT_CONTRACT_CODE.clone()),
            HashMap::new(),
        );

        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(Hardfork::Cancun, ForkCondition::Timestamp(1))
                .build(),
        );

        // execute invalid header (no parent beacon block root)
        let mut executor = EVMProcessor::new_with_db(chain_spec, StateProviderDatabase::new(db));

        // attempt to execute a block without parent beacon block root, expect err
        let err = executor
            .execute_and_verify_receipt(
                &BlockWithSenders {
                    block: Block {
                        header: header.clone(),
                        body: vec![],
                        ommers: vec![],
                        withdrawals: None,
                    },
                    senders: vec![],
                },
                U256::ZERO,
            )
            .expect_err(
                "Executing cancun block without parent beacon block root field should fail",
            );
        assert_eq!(
            err,
            BlockExecutionError::Validation(BlockValidationError::MissingParentBeaconBlockRoot)
        );

        // fix header, set a gas limit
        header.parent_beacon_block_root = Some(B256::with_last_byte(0x69));

        // Now execute a block with the fixed header, ensure that it does not fail
        executor
            .execute(
                &BlockWithSenders {
                    block: Block {
                        header: header.clone(),
                        body: vec![],
                        ommers: vec![],
                        withdrawals: None,
                    },
                    senders: vec![],
                },
                U256::ZERO,
            )
            .unwrap();

        // check the actual storage of the contract - it should be:
        // * The storage value at header.timestamp % HISTORY_BUFFER_LENGTH should be
        // header.timestamp
        // * The storage value at header.timestamp % HISTORY_BUFFER_LENGTH + HISTORY_BUFFER_LENGTH
        // should be parent_beacon_block_root
        let history_buffer_length = 8191u64;
        let timestamp_index = header.timestamp % history_buffer_length;
        let parent_beacon_block_root_index =
            timestamp_index % history_buffer_length + history_buffer_length;

        // get timestamp storage and compare
        let timestamp_storage =
            executor.db_mut().storage(BEACON_ROOTS_ADDRESS, U256::from(timestamp_index)).unwrap();
        assert_eq!(timestamp_storage, U256::from(header.timestamp));

        // get parent beacon block root storage and compare
        let parent_beacon_block_root_storage = executor
            .db_mut()
            .storage(BEACON_ROOTS_ADDRESS, U256::from(parent_beacon_block_root_index))
            .expect("storage value should exist");
        assert_eq!(parent_beacon_block_root_storage, U256::from(0x69));
    }

    #[test]
    fn eip_4788_no_code_cancun() {
        // This test ensures that we "silently fail" when cancun is active and there is no code at
        // BEACON_ROOTS_ADDRESS
        let header = Header {
            timestamp: 1,
            number: 1,
            parent_beacon_block_root: Some(B256::with_last_byte(0x69)),
            excess_blob_gas: Some(0),
            ..Header::default()
        };

        let db = StateProviderTest::default();

        // DON'T deploy the contract at genesis
        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(Hardfork::Cancun, ForkCondition::Timestamp(1))
                .build(),
        );

        let mut executor = EVMProcessor::new_with_db(chain_spec, StateProviderDatabase::new(db));
        executor.init_env(&header, U256::ZERO);

        // get the env
        let previous_env = executor.evm.env.clone();

        // attempt to execute an empty block with parent beacon block root, this should not fail
        executor
            .execute_and_verify_receipt(
                &BlockWithSenders {
                    block: Block {
                        header: header.clone(),
                        body: vec![],
                        ommers: vec![],
                        withdrawals: None,
                    },
                    senders: vec![],
                },
                U256::ZERO,
            )
            .expect(
                "Executing a block with no transactions while cancun is active should not fail",
            );

        // ensure that the env has not changed
        assert_eq!(executor.evm.env, previous_env);
    }

    #[test]
    fn eip_4788_empty_account_call() {
        // This test ensures that we do not increment the nonce of an empty SYSTEM_ADDRESS account
        // during the pre-block call
        let mut db = StateProviderTest::default();

        let beacon_root_contract_account = Account {
            balance: U256::ZERO,
            bytecode_hash: Some(keccak256(BEACON_ROOT_CONTRACT_CODE.clone())),
            nonce: 1,
        };

        db.insert_account(
            BEACON_ROOTS_ADDRESS,
            beacon_root_contract_account,
            Some(BEACON_ROOT_CONTRACT_CODE.clone()),
            HashMap::new(),
        );

        // insert an empty SYSTEM_ADDRESS
        db.insert_account(SYSTEM_ADDRESS, Account::default(), None, HashMap::new());

        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(Hardfork::Cancun, ForkCondition::Timestamp(1))
                .build(),
        );

        let mut executor = EVMProcessor::new_with_db(chain_spec, StateProviderDatabase::new(db));

        // construct the header for block one
        let header = Header {
            timestamp: 1,
            number: 1,
            parent_beacon_block_root: Some(B256::with_last_byte(0x69)),
            excess_blob_gas: Some(0),
            ..Header::default()
        };

        executor.init_env(&header, U256::ZERO);

        // attempt to execute an empty block with parent beacon block root, this should not fail
        executor
            .execute_and_verify_receipt(
                &BlockWithSenders {
                    block: Block {
                        header: header.clone(),
                        body: vec![],
                        ommers: vec![],
                        withdrawals: None,
                    },
                    senders: vec![],
                },
                U256::ZERO,
            )
            .expect(
                "Executing a block with no transactions while cancun is active should not fail",
            );

        // ensure that the nonce of the system address account has not changed
        let nonce = executor.db_mut().basic(SYSTEM_ADDRESS).unwrap().unwrap().nonce;
        assert_eq!(nonce, 0);
    }

    #[test]
    fn eip_4788_genesis_call() {
        let mut db = StateProviderTest::default();

        let beacon_root_contract_account = Account {
            balance: U256::ZERO,
            bytecode_hash: Some(keccak256(BEACON_ROOT_CONTRACT_CODE.clone())),
            nonce: 1,
        };

        db.insert_account(
            BEACON_ROOTS_ADDRESS,
            beacon_root_contract_account,
            Some(BEACON_ROOT_CONTRACT_CODE.clone()),
            HashMap::new(),
        );

        // activate cancun at genesis
        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(Hardfork::Cancun, ForkCondition::Timestamp(0))
                .build(),
        );

        let mut header = chain_spec.genesis_header();

        let mut executor = EVMProcessor::new_with_db(chain_spec, StateProviderDatabase::new(db));
        executor.init_env(&header, U256::ZERO);

        // attempt to execute the genesis block with non-zero parent beacon block root, expect err
        header.parent_beacon_block_root = Some(B256::with_last_byte(0x69));
        let _err = executor
            .execute_and_verify_receipt(
                &BlockWithSenders {
                    block: Block {
                        header: header.clone(),
                        body: vec![],
                        ommers: vec![],
                        withdrawals: None,
                    },
                    senders: vec![],
                },
                U256::ZERO,
            )
            .expect_err(
                "Executing genesis cancun block with non-zero parent beacon block root field should fail",
            );

        // fix header
        header.parent_beacon_block_root = Some(B256::ZERO);

        // now try to process the genesis block again, this time ensuring that a system contract
        // call does not occur
        executor
            .execute(
                &BlockWithSenders {
                    block: Block {
                        header: header.clone(),
                        body: vec![],
                        ommers: vec![],
                        withdrawals: None,
                    },
                    senders: vec![],
                },
                U256::ZERO,
            )
            .unwrap();

        // there is no system contract call so there should be NO STORAGE CHANGES
        // this means we'll check the transition state
        let state = executor.evm.db().unwrap();
        let transition_state = state
            .transition_state
            .clone()
            .expect("the evm should be initialized with bundle updates");

        // assert that it is the default (empty) transition state
        assert_eq!(transition_state, TransitionState::default());
    }

    #[test]
    fn eip_4788_high_base_fee() {
        // This test ensures that if we have a base fee, then we don't return an error when the
        // system contract is called, due to the gas price being less than the base fee.
        let header = Header {
            timestamp: 1,
            number: 1,
            parent_beacon_block_root: Some(B256::with_last_byte(0x69)),
            base_fee_per_gas: Some(u64::MAX),
            excess_blob_gas: Some(0),
            ..Header::default()
        };

        let mut db = StateProviderTest::default();

        let beacon_root_contract_account = Account {
            balance: U256::ZERO,
            bytecode_hash: Some(keccak256(BEACON_ROOT_CONTRACT_CODE.clone())),
            nonce: 1,
        };

        db.insert_account(
            BEACON_ROOTS_ADDRESS,
            beacon_root_contract_account,
            Some(BEACON_ROOT_CONTRACT_CODE.clone()),
            HashMap::new(),
        );

        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(Hardfork::Cancun, ForkCondition::Timestamp(1))
                .build(),
        );

        // execute header
        let mut executor = EVMProcessor::new_with_db(chain_spec, StateProviderDatabase::new(db));
        executor.init_env(&header, U256::ZERO);

        // ensure that the env is configured with a base fee
        assert_eq!(executor.evm.env.block.basefee, U256::from(u64::MAX));

        // Now execute a block with the fixed header, ensure that it does not fail
        executor
            .execute(
                &BlockWithSenders {
                    block: Block {
                        header: header.clone(),
                        body: vec![],
                        ommers: vec![],
                        withdrawals: None,
                    },
                    senders: vec![],
                },
                U256::ZERO,
            )
            .unwrap();

        // check the actual storage of the contract - it should be:
        // * The storage value at header.timestamp % HISTORY_BUFFER_LENGTH should be
        // header.timestamp
        // * The storage value at header.timestamp % HISTORY_BUFFER_LENGTH + HISTORY_BUFFER_LENGTH
        // should be parent_beacon_block_root
        let history_buffer_length = 8191u64;
        let timestamp_index = header.timestamp % history_buffer_length;
        let parent_beacon_block_root_index =
            timestamp_index % history_buffer_length + history_buffer_length;

        // get timestamp storage and compare
        let timestamp_storage =
            executor.db_mut().storage(BEACON_ROOTS_ADDRESS, U256::from(timestamp_index)).unwrap();
        assert_eq!(timestamp_storage, U256::from(header.timestamp));

        // get parent beacon block root storage and compare
        let parent_beacon_block_root_storage = executor
            .db_mut()
            .storage(BEACON_ROOTS_ADDRESS, U256::from(parent_beacon_block_root_index))
            .unwrap();
        assert_eq!(parent_beacon_block_root_storage, U256::from(0x69));
    }
}

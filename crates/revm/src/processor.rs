use crate::{
    database::RevmDatabase,
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
    Address, Block, BlockNumber, Bloom, ChainSpec, Hardfork, Header, PruneModes, Receipt,
    ReceiptWithBloom, TransactionSigned, H256, U256,
};
use reth_provider::{
    change::BundleStateWithReceipts, BlockExecutor, BlockExecutorStats, StateProvider,
};
use reth_revm_primitives::env::fill_tx_env_with_beacon_root_contract_call;
use revm::{
    db::{states::bundle_state::BundleRetention, StateDBBox},
    primitives::ResultAndState,
    DatabaseCommit, State, EVM,
};
use std::{sync::Arc, time::Instant};
use tracing::{debug, trace};

/// Main block executor
pub struct EVMProcessor<'a> {
    /// The configured chain-spec
    chain_spec: Arc<ChainSpec>,
    evm: EVM<StateDBBox<'a, Error>>,
    stack: InspectorStack,
    receipts: Vec<Vec<Receipt>>,
    /// First block will be initialized to ZERO
    /// and be set to the block number of first block executed.
    first_block: Option<BlockNumber>,
    /// The maximum known block .
    tip: Option<BlockNumber>,
    /// Pruning configuration.
    prune_modes: PruneModes,
    /// Execution stats
    stats: BlockExecutorStats,
}

impl<'a> EVMProcessor<'a> {
    /// Return chain spec.
    pub fn chain_spec(&self) -> &Arc<ChainSpec> {
        &self.chain_spec
    }

    /// Create new Processor wit given chain spec.
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
            stats: BlockExecutorStats::default(),
        }
    }

    /// Creates a new executor from the given chain spec and database.
    pub fn new_with_db<DB: StateProvider + 'a>(
        chain_spec: Arc<ChainSpec>,
        db: RevmDatabase<DB>,
    ) -> Self {
        let state = State::builder()
            .with_database_boxed(Box::new(db))
            .with_bundle_update()
            .without_state_clear()
            .build();
        EVMProcessor::new_with_state(chain_spec, state)
    }

    /// Create new EVM processor with a given revm state.
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
            stats: BlockExecutorStats::default(),
        }
    }

    /// Configures the executor with the given inspectors.
    pub fn set_stack(&mut self, stack: InspectorStack) {
        self.stack = stack;
    }

    /// Gives a reference to the database
    pub fn db(&mut self) -> &mut StateDBBox<'a, Error> {
        self.evm.db().expect("db to not be moved")
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
        //set state clear flag
        self.evm.db.as_mut().unwrap().set_state_clear_flag(
            self.chain_spec.fork(Hardfork::SpuriousDragon).active_at_block(header.number),
        );

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
    pub fn apply_pre_block_call(&mut self, block: &Block) -> Result<(), BlockExecutionError> {
        if self.chain_spec.fork(Hardfork::Cancun).active_at_timestamp(block.timestamp) {
            // if the block number is zero (genesis block) then the parent beacon block root must
            // be 0x0 and no system transaction may occur as per EIP-4788
            if block.number == 0 {
                if block.parent_beacon_block_root != Some(H256::zero()) {
                    return Err(
                        BlockValidationError::CancunGenesisParentBeaconBlockRootNotZero.into()
                    )
                }
            } else {
                let parent_beacon_block_root = block.parent_beacon_block_root.ok_or(
                    BlockExecutionError::from(BlockValidationError::MissingParentBeaconBlockRoot),
                )?;
                fill_tx_env_with_beacon_root_contract_call(
                    &mut self.evm.env.tx,
                    parent_beacon_block_root,
                );

                let ResultAndState { state, .. } = self.evm.transact().map_err(|e| {
                    BlockExecutionError::from(BlockValidationError::EVM {
                        hash: Default::default(),
                        message: format!("{e:?}"),
                    })
                })?;

                self.db().commit(state);
            }
        }
        Ok(())
    }

    /// Post execution state change that include block reward, withdrawals and iregular DAO hardfork
    /// state change.
    pub fn post_execution_state_change(
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
                .db()
                .drain_balances(DAO_HARDKFORK_ACCOUNTS)
                .map_err(|_| BlockValidationError::IncrementBalanceFailed)?
                .into_iter()
                .sum();

            // return balance to DAO beneficiary.
            *balance_increments.entry(DAO_HARDFORK_BENEFICIARY).or_default() += drained_balance;
        }
        // increment balances
        self.db()
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
    ) -> Result<u64, BlockExecutionError> {
        self.init_env(&block.header, total_difficulty);
        self.apply_pre_block_call(block)?;

        // perf: do not execute empty blocks
        if block.body.is_empty() {
            self.receipts.push(Vec::new());
            return Ok(0)
        }
        let senders = self.recover_senders(&block.body, senders)?;

        let mut cumulative_gas_used = 0;
        let mut receipts = Vec::with_capacity(block.body.len());
        for (transaction, sender) in block.body.iter().zip(senders) {
            let time = Instant::now();
            // The sum of the transaction’s gas limit, Tg, and the gas utilised in this block prior,
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

            self.db().commit(state);

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
        self.receipts.push(receipts);

        Ok(cumulative_gas_used)
    }
}

impl<'a> BlockExecutor for EVMProcessor<'a> {
    fn execute(
        &mut self,
        block: &Block,
        total_difficulty: U256,
        senders: Option<Vec<Address>>,
    ) -> Result<(), BlockExecutionError> {
        let cumulative_gas_used = self.execute_transactions(block, total_difficulty, senders)?;

        // Check if gas used matches the value set in header.
        if block.gas_used != cumulative_gas_used {
            return Err(BlockValidationError::BlockGasUsed {
                got: cumulative_gas_used,
                expected: block.gas_used,
                receipts: self
                    .receipts
                    .last()
                    .map(|block_r| {
                        block_r
                            .iter()
                            .enumerate()
                            .map(|(id, tx_r)| (id as u64, tx_r.cumulative_gas_used))
                            .collect()
                    })
                    .unwrap_or_default(),
            }
            .into())
        }
        let time = Instant::now();
        self.post_execution_state_change(block, total_difficulty)?;
        self.stats.apply_post_execution_changes_duration += time.elapsed();

        let time = Instant::now();
        let retention = if self.tip.map_or(true, |tip| {
            !self.prune_modes.should_prune_account_history(block.number, tip) &&
                !self.prune_modes.should_prune_storage_history(block.number, tip)
        }) {
            BundleRetention::Reverts
        } else {
            BundleRetention::PlainState
        };
        self.db().merge_transitions(retention);
        self.stats.merge_transitions_duration += time.elapsed();

        if self.first_block.is_none() {
            self.first_block = Some(block.number);
        }
        Ok(())
    }

    fn execute_and_verify_receipt(
        &mut self,
        block: &Block,
        total_difficulty: U256,
        senders: Option<Vec<Address>>,
    ) -> Result<(), BlockExecutionError> {
        // execute block
        self.execute(block, total_difficulty, senders)?;

        // TODO Before Byzantium, receipts contained state root that would mean that expensive
        // operation as hashing that is needed for state root got calculated in every
        // transaction This was replaced with is_success flag.
        // See more about EIP here: https://eips.ethereum.org/EIPS/eip-658
        if self.chain_spec.fork(Hardfork::Byzantium).active_at_block(block.header.number) {
            let time = Instant::now();
            if let Err(e) = verify_receipt(
                block.header.receipts_root,
                block.header.logs_bloom,
                self.receipts.last().unwrap().iter(),
            ) {
                debug!(
                    target = "sync",
                    "receipts verification failed {:?} receipts:{:?}",
                    e,
                    self.receipts.last().unwrap()
                );
                return Err(e)
            };
            self.stats.receipt_root_duration += time.elapsed();
        }

        Ok(())
    }

    fn set_prune_modes(&mut self, prune_modes: PruneModes) {
        self.prune_modes = prune_modes;
    }

    fn set_tip(&mut self, tip: BlockNumber) {
        self.tip = Some(tip);
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

#[cfg(test)]
mod tests {
    use reth_interfaces::test_utils::generators::sign_tx_with_key_pair;
    use reth_primitives::{
        constants::BEACON_ROOTS_ADDRESS, keccak256, public_key_to_address, Account, Bytecode,
        Bytes, ChainSpecBuilder, ForkCondition, StorageKey, Transaction, TransactionKind, TxLegacy,
        MAINNET,
    };
    use reth_provider::{AccountReader, BlockHashReader, StateRootProvider};
    use reth_revm_primitives::TransitionState;
    use secp256k1::{rand, KeyPair, Secp256k1};
    use std::{collections::HashMap, str::FromStr};

    use super::*;

    #[derive(Debug, Default, Clone, Eq, PartialEq)]
    struct StateProviderTest {
        accounts: HashMap<Address, (HashMap<StorageKey, U256>, Account)>,
        contracts: HashMap<H256, Bytecode>,
        block_hash: HashMap<u64, H256>,
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
                self.contracts.insert(hash, Bytecode::new_raw(bytecode.into()));
            }
            self.accounts.insert(address, (storage, account));
        }
    }

    impl AccountReader for StateProviderTest {
        fn basic_account(&self, address: Address) -> reth_interfaces::Result<Option<Account>> {
            let ret = Ok(self.accounts.get(&address).map(|(_, acc)| *acc));
            ret
        }
    }

    impl BlockHashReader for StateProviderTest {
        fn block_hash(&self, number: u64) -> reth_interfaces::Result<Option<H256>> {
            Ok(self.block_hash.get(&number).cloned())
        }

        fn canonical_hashes_range(
            &self,
            start: BlockNumber,
            end: BlockNumber,
        ) -> reth_interfaces::Result<Vec<H256>> {
            let range = start..end;
            Ok(self
                .block_hash
                .iter()
                .filter_map(|(block, hash)| range.contains(block).then_some(*hash))
                .collect())
        }
    }

    impl StateRootProvider for StateProviderTest {
        fn state_root(
            &self,
            _bundle_state: BundleStateWithReceipts,
        ) -> reth_interfaces::Result<H256> {
            todo!()
        }
    }

    impl StateProvider for StateProviderTest {
        fn storage(
            &self,
            account: Address,
            storage_key: reth_primitives::StorageKey,
        ) -> reth_interfaces::Result<Option<reth_primitives::StorageValue>> {
            Ok(self
                .accounts
                .get(&account)
                .and_then(|(storage, _)| storage.get(&storage_key).cloned()))
        }

        fn bytecode_by_hash(&self, code_hash: H256) -> reth_interfaces::Result<Option<Bytecode>> {
            Ok(self.contracts.get(&code_hash).cloned())
        }

        fn proof(
            &self,
            _address: Address,
            _keys: &[H256],
        ) -> reth_interfaces::Result<(Vec<Bytes>, H256, Vec<Vec<Bytes>>)> {
            todo!()
        }
    }

    #[test]
    fn eip_4788_non_genesis_call() {
        let mut header = Header { timestamp: 1, number: 1, ..Header::default() };

        let mut db = StateProviderTest::default();

        // set up key that we can use to construct a tx, which actually calls the beacon root
        // contract
        let secp = Secp256k1::new();
        let key_pair = KeyPair::new(&secp, &mut rand::thread_rng());
        let caller_address = public_key_to_address(key_pair.public_key());

        let caller_genesis = Account { balance: U256::MAX, ..Default::default() };
        db.insert_account(caller_address, caller_genesis, None, HashMap::new());

        let beacon_root_contract_code = Bytes::from_str("0x3373fffffffffffffffffffffffffffffffffffffffe14604457602036146024575f5ffd5b620180005f350680545f35146037575f5ffd5b6201800001545f5260205ff35b42620180004206555f3562018000420662018000015500").unwrap();

        let beacon_root_contract_account = Account {
            balance: U256::ZERO,
            bytecode_hash: Some(keccak256(beacon_root_contract_code.clone())),
            nonce: 1,
        };

        db.insert_account(
            BEACON_ROOTS_ADDRESS,
            beacon_root_contract_account,
            Some(beacon_root_contract_code),
            HashMap::new(),
        );

        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .shanghai_activated()
                .with_fork(Hardfork::Cancun, ForkCondition::Timestamp(1))
                .build(),
        );

        // TODO: seems like this is not possible because the type built from this is not State<'a,
        // reth_interfaces::Error>, it is State<'a, Infallible>, which is incompatible with the
        // EVMProcessor.
        // let mut state = StateBuilder::default().with_cached_prestate(cache_state).build();

        // execute chain and verify receipts
        // let mut executor = EVMProcessor::new_with_state(chain_spec, state);
        let mut executor = EVMProcessor::new_with_db(chain_spec, RevmDatabase::new(db));

        // attempt to execute a block without parent beacon block root, expect err
        let _err = executor
            .execute_and_verify_receipt(
                &Block { header: header.clone(), body: vec![], ommers: vec![], withdrawals: None },
                U256::ZERO,
                None,
            )
            .expect_err(
                "Executing cancun block without parent beacon block root field should fail",
            );

        // fix header, set a gas limit
        header.parent_beacon_block_root = Some(H256::from_low_u64_be(0x1337));
        header.gas_limit = u64::MAX;
        header.gas_used = 25440;

        // build and sign transaction that calls the beacon root contract
        let transaction = Transaction::Legacy(TxLegacy {
            chain_id: Some(1),
            nonce: 0,
            gas_price: 1,
            gas_limit: u64::MAX,
            to: TransactionKind::Call(BEACON_ROOTS_ADDRESS),
            value: 0,
            // caller must specify timestamp in calldata, it must be 32 bytes
            input: Bytes::from(H256::from_low_u64_be(header.timestamp).to_fixed_bytes()),
        });

        let signed_tx = sign_tx_with_key_pair(key_pair, transaction);

        // Now execute a block with a tx that calls the beacon root contract, ensure that it does
        // not fail
        executor
            .execute(
                &Block {
                    header: header.clone(),
                    body: vec![signed_tx],
                    ommers: vec![],
                    withdrawals: None,
                },
                U256::ZERO,
                Some(vec![caller_address]),
            )
            .unwrap();

        // check the receipts to ensure that the transaction didn't fail
        let receipts = executor
            .receipts
            .last()
            .expect("there should be receipts for the block we just executed");
        assert_eq!(receipts.len(), 1);
        let receipt = receipts.first().expect("there is one receipt");
        assert!(receipt.success);

        // check the actual storage of the contract - it should be:
        // * The storage value at header.timestamp % HISTORY_BUFFER_LENGTH should be
        // header.timestamp
        // * The storage value at header.timestamp % HISTORY_BUFFER_LENGTH + HISTORY_BUFFER_LENGTH
        // should be parent_beacon_block_root
        let history_buffer_length = 98304u64;
        let timestamp_index = header.timestamp % history_buffer_length;
        let parent_beacon_block_root_index =
            timestamp_index % history_buffer_length + history_buffer_length;

        // get timestamp storage and compare
        let timestamp_storage = executor
            .db()
            .database
            .storage(BEACON_ROOTS_ADDRESS, U256::from(timestamp_index))
            .unwrap();
        assert_eq!(timestamp_storage, U256::from(header.timestamp));

        // get parent beacon block root storage and compare
        let parent_beacon_block_root_storage = executor
            .db()
            .database
            .storage(BEACON_ROOTS_ADDRESS, U256::from(parent_beacon_block_root_index))
            .unwrap();
        assert_eq!(parent_beacon_block_root_storage, U256::from(0x1337));
    }

    #[test]
    fn eip_4788_no_code_cancun() {
        // This test ensures that we "silently fail" when cancun is active and there is no code at
        // BEACON_ROOTS_ADDRESS
        let header = Header {
            timestamp: 1,
            number: 1,
            parent_beacon_block_root: Some(H256::from_low_u64_be(0x1337)),
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

        let mut executor = EVMProcessor::new_with_db(chain_spec, RevmDatabase::new(db));

        // attempt to execute a block without parent beacon block root, expect err
        let _out = executor
            .execute_and_verify_receipt(
                &Block { header: header.clone(), body: vec![], ommers: vec![], withdrawals: None },
                U256::ZERO,
                None,
            )
            .expect(
                "Executing a block with no transactions while cancun is active should not fail",
            );
    }

    #[test]
    fn eip_4788_genesis_call() {
        let mut db = StateProviderTest::default();

        let beacon_root_contract_code = Bytes::from_str("0x3373fffffffffffffffffffffffffffffffffffffffe14604457602036146024575f5ffd5b620180005f350680545f35146037575f5ffd5b6201800001545f5260205ff35b42620180004206555f3562018000420662018000015500").unwrap();

        let beacon_root_contract_account = Account {
            balance: U256::ZERO,
            bytecode_hash: Some(keccak256(beacon_root_contract_code.clone())),
            nonce: 1,
        };
        db.insert_account(
            BEACON_ROOTS_ADDRESS,
            beacon_root_contract_account,
            Some(beacon_root_contract_code),
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

        let mut executor = EVMProcessor::new_with_db(chain_spec, RevmDatabase::new(db));

        // attempt to execute the genesis block with non-zero parent beacon block root, expect err
        header.parent_beacon_block_root = Some(H256::from_low_u64_be(0x1337));
        let _err = executor
            .execute_and_verify_receipt(
                &Block { header: header.clone(), body: vec![], ommers: vec![], withdrawals: None },
                U256::ZERO,
                None,
            )
            .expect_err(
                "Executing genesis cancun block with non-zero parent beacon block root field should fail",
            );

        // fix header
        header.parent_beacon_block_root = Some(H256::zero());

        // now try to process the genesis block again, this time ensuring that a system contract
        // call does not occur
        let out = executor
            .execute(
                &Block { header: header.clone(), body: vec![], ommers: vec![], withdrawals: None },
                U256::ZERO,
                None,
            )
            .unwrap();

        // there is no system contract call so there should be NO STORAGE CHANGES
        // this means we'll check the transition state
        let state = executor.evm.db().unwrap();
        let transition_state =
            state.transition_state.expect("the evm should be initialized with bundle updates");

        // assert that it is the default (empty) transition state
        assert_eq!(transition_state, TransitionState::default());
    }
}

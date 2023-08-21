use crate::{
    database::State,
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
    Address, Block, BlockNumber, Bloom, ChainSpec, Hardfork, Header, Receipt, ReceiptWithBloom,
    TransactionSigned, H256, U256,
};
use reth_provider::{change::BundleState, BlockExecutor, BlockExecutorStats, StateProvider};
use revm::{
    primitives::ResultAndState, DatabaseCommit, State as RevmState,
    StateBuilder as RevmStateBuilder, EVM,
};
use std::{sync::Arc, time::Instant};
use tracing::{debug, trace};

/// Main block executor
pub struct EVMProcessor<'a> {
    /// The configured chain-spec
    pub chain_spec: Arc<ChainSpec>,
    evm: EVM<RevmState<'a, Error>>,
    stack: InspectorStack,
    receipts: Vec<Vec<Receipt>>,
    /// First block will be initialized to ZERO
    /// and be set to the block number of first block executed.
    first_block: BlockNumber,
    /// Execution stats
    stats: BlockExecutorStats,
}

impl<'a> From<Arc<ChainSpec>> for EVMProcessor<'a> {
    /// Instantiates a new executor from the chainspec. Must call
    /// `with_db` to set the database before executing.
    fn from(chain_spec: Arc<ChainSpec>) -> Self {
        let evm = EVM::new();
        EVMProcessor {
            chain_spec,
            evm,
            stack: InspectorStack::new(InspectorStackConfig::default()),
            receipts: Vec::new(),
            first_block: 0,
            stats: BlockExecutorStats::default(),
        }
    }
}

impl<'a> EVMProcessor<'a> {
    /// Creates a new executor from the given chain spec and database.
    pub fn new<DB: StateProvider + 'a>(chain_spec: Arc<ChainSpec>, db: State<DB>) -> Self {
        let revm_state =
            RevmStateBuilder::default().with_database(Box::new(db)).without_state_clear().build();
        EVMProcessor::new_with_revm_state(chain_spec, revm_state)
    }

    /// Create new EVM processor with a given revm state.
    pub fn new_with_revm_state(
        chain_spec: Arc<ChainSpec>,
        revm_state: RevmState<'a, Error>,
    ) -> Self {
        let mut evm = EVM::new();
        evm.database(revm_state);
        EVMProcessor {
            chain_spec,
            evm,
            stack: InspectorStack::new(InspectorStackConfig::default()),
            receipts: Vec::new(),
            first_block: 0,
            stats: BlockExecutorStats::default(),
        }
    }

    /// Create new EVM processor with a given bundle state.
    pub fn new_with_state<DB: StateProvider + 'a>(
        chain_spec: Arc<ChainSpec>,
        db: State<DB>,
        state: BundleState,
    ) -> Self {
        let mut evm = EVM::new();
        let (bundle_state, receipts, block_number, block_hashes) = state.into_inner();
        let revm_state = RevmStateBuilder::default()
            .with_database(Box::new(db))
            .with_bundle_prestate(bundle_state)
            .with_block_hashes(block_hashes)
            .build();
        evm.database(revm_state);
        EVMProcessor {
            chain_spec,
            evm,
            stack: InspectorStack::new(InspectorStackConfig::default()),
            receipts,
            first_block: block_number,
            stats: BlockExecutorStats::default(),
        }
    }

    /// Configures the executor with the given inspectors.
    pub fn set_stack(&mut self, stack: InspectorStack) {
        self.stack = stack;
    }

    /// Gives a reference to the database
    pub fn db(&mut self) -> &mut RevmState<'a, Error> {
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
    /// The returned [PostState] can be used to persist the changes to disk, and contains the
    /// changes made by each transaction.
    ///
    /// The changes in [PostState] have a transition ID associated with them: there is one
    /// transition ID for each transaction (with the first executed tx having transition ID 0, and
    /// so on).
    ///
    /// The second returned value represents the total gas used by this block of transactions.
    pub fn execute_transactions(
        &mut self,
        block: &Block,
        total_difficulty: U256,
        senders: Option<Vec<Address>>,
    ) -> Result<u64, BlockExecutionError> {
        // perf: do not execute empty blocks
        if block.body.is_empty() {
            self.receipts.push(Vec::new());
            return Ok(0)
        }
        let senders = self.recover_senders(&block.body, senders)?;

        self.init_env(&block.header, total_difficulty);

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
        self.db().merge_transitions();
        self.stats.merge_transitions_duration += time.elapsed();

        if self.first_block == 0 {
            self.first_block = block.number;
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

    fn take_output_state(&mut self) -> BundleState {
        let receipts = std::mem::take(&mut self.receipts);
        BundleState::new(self.evm.db().unwrap().take_bundle(), receipts, self.first_block)
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

//TODO(rakita) tests
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{database::State, state_change};
    use once_cell::sync::Lazy;
    use reth_consensus_common::calc;
    use reth_primitives::{
        constants::ETH_TO_WEI, hex_literal::hex, keccak256, Account, Address, BlockNumber,
        Bytecode, Bytes, ChainSpecBuilder, ForkCondition, StorageKey, H256, MAINNET, U256,
    };
    use reth_provider::{
        //post_state::{AccountChanges, Storage, StorageTransition, StorageWipe},
        AccountReader,
        BlockHashReader,
        StateProvider,
        StateRootProvider,
    };
    use reth_rlp::Decodable;
    use std::{
        collections::{hash_map, HashMap},
        str::FromStr,
    };

    /*
        static DEFAULT_REVM_ACCOUNT: Lazy<RevmAccount> = Lazy::new(|| RevmAccount {
            info: AccountInfo::default(),
            storage: hash_map::HashMap::default(),
            is_destroyed: false,
            is_touched: false,
            storage_cleared: false,
            is_not_existing: false,
        });
    */
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
        fn state_root(&self, _post_state: BundleState) -> reth_interfaces::Result<H256> {
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

    /* SANITY Execution
    #[test]
    fn sanity_execution() {
        // Got rlp block from: src/GeneralStateTestsFiller/stChainId/chainIdGasCostFiller.json

        let mut block_rlp = hex!("f90262f901f9a075c371ba45999d87f4542326910a11af515897aebce5265d3f6acd1f1161f82fa01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa098f2dcd87c8ae4083e7017a05456c14eea4b1db2032126e27b3b1563d57d7cc0a08151d548273f6683169524b66ca9fe338b9ce42bc3540046c828fd939ae23bcba03f4e5c2ec5b2170b711d97ee755c160457bb58d8daa338e835ec02ae6860bbabb901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000083020000018502540be40082a8798203e800a00000000000000000000000000000000000000000000000000000000000000000880000000000000000f863f861800a8405f5e10094100000000000000000000000000000000000000080801ba07e09e26678ed4fac08a249ebe8ed680bf9051a5e14ad223e4b2b9d26e0208f37a05f6e3f188e3e6eab7d7d3b6568f5eac7d687b08d307d3154ccd8c87b4630509bc0").as_slice();
        let mut block = Block::decode(&mut block_rlp).unwrap();

        let mut ommer = Header::default();
        let ommer_beneficiary =
            Address::from_str("3000000000000000000000000000000000000000").unwrap();
        ommer.beneficiary = ommer_beneficiary;
        ommer.number = block.number;
        block.ommers = vec![ommer];

        let mut db = StateProviderTest::default();

        let account1 = Address::from_str("1000000000000000000000000000000000000000").unwrap();
        let account2 = Address::from_str("2adc25665018aa1fe0e6bc666dac8fc2697ff9ba").unwrap();
        let account3 = Address::from_str("a94f5374fce5edbc8e2a8697c15331677e6ebf0b").unwrap();

        // pre state
        db.insert_account(
            account1,
            Account { balance: U256::ZERO, nonce: 0x00, bytecode_hash: None },
            Some(hex!("5a465a905090036002900360015500").into()),
            HashMap::new(),
        );

        let account3_old_info = Account {
            balance: U256::from(0x3635c9adc5dea00000u128),
            nonce: 0x00,
            bytecode_hash: None,
        };

        db.insert_account(
            account3,
            Account {
                balance: U256::from(0x3635c9adc5dea00000u128),
                nonce: 0x00,
                bytecode_hash: None,
            },
            None,
            HashMap::new(),
        );

        // spec at berlin fork
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().berlin_activated().build());

        let db = SubState::new(State::new(db));

        // execute chain and verify receipts
        let mut executor = Executor::new(chain_spec, db);
        let post_state = executor.execute_and_verify_receipt(&block, U256::ZERO, None).unwrap();

        let base_block_reward = ETH_TO_WEI * 2;
        let block_reward = calc::block_reward(base_block_reward, 1);

        let account1_info = Account { balance: U256::ZERO, nonce: 0x00, bytecode_hash: None };
        let account2_info = Account {
            // Block reward decrease
            balance: U256::from(0x1bc16d674ece94bau128 - 0x1bc16d674ec80000u128),
            nonce: 0x00,
            bytecode_hash: None,
        };
        let account2_info_with_block_reward = Account {
            balance: account2_info.balance + block_reward,
            nonce: 0x00,
            bytecode_hash: None,
        };
        let account3_info = Account {
            balance: U256::from(0x3635c9adc5de996b46u128),
            nonce: 0x01,
            bytecode_hash: None,
        };
        let ommer_beneficiary_info = Account {
            nonce: 0,
            balance: U256::from(calc::ommer_reward(
                base_block_reward,
                block.number,
                block.ommers[0].number,
            )),
            bytecode_hash: None,
        };

        // Check if cache is set
        // account1
        let db = executor.db();
        let cached_acc1 = db.accounts.get(&account1).unwrap();
        assert_eq!(cached_acc1.info.balance, account1_info.balance);
        assert_eq!(cached_acc1.info.nonce, account1_info.nonce);
        assert_eq!(cached_acc1.account_state, AccountState::Touched);
        assert_eq!(cached_acc1.storage.len(), 1);
        assert_eq!(cached_acc1.storage.get(&U256::from(1)), Some(&U256::from(2)));

        // account2 Block reward
        let cached_acc2 = db.accounts.get(&account2).unwrap();
        assert_eq!(cached_acc2.info.balance, account2_info.balance + block_reward);
        assert_eq!(cached_acc2.info.nonce, account2_info.nonce);
        assert_eq!(cached_acc2.account_state, AccountState::Touched);
        assert_eq!(cached_acc2.storage.len(), 0);

        // account3
        let cached_acc3 = db.accounts.get(&account3).unwrap();
        assert_eq!(cached_acc3.info.balance, account3_info.balance);
        assert_eq!(cached_acc3.info.nonce, account3_info.nonce);
        assert_eq!(cached_acc3.account_state, AccountState::Touched);
        assert_eq!(cached_acc3.storage.len(), 0);

        assert!(
            post_state.accounts().get(&account1).is_none(),
            "Account should not be present in post-state since it was not changed"
        );

        // Clone and sort to make the test deterministic
        assert_eq!(
            post_state.account_changes().inner,
            BTreeMap::from([(
                block.number,
                BTreeMap::from([
                    // New account
                    (account2, None),
                    // Changed account
                    (account3, Some(account3_old_info)),
                    // Ommer reward
                    (ommer_beneficiary, None)
                ])
            ),]),
            "Account changeset did not match"
        );
        assert_eq!(
            post_state.storage_changes().inner,
            BTreeMap::from([(
                block.number,
                BTreeMap::from([(
                    account1,
                    StorageTransition {
                        wipe: StorageWipe::None,
                        // Slot 1 changed from 0 to 2
                        storage: BTreeMap::from([(U256::from(1), U256::ZERO)])
                    }
                )])
            )]),
            "Storage changeset did not match"
        );

        // Check final post-state
        assert_eq!(
            post_state.storage(),
            &BTreeMap::from([(
                account1,
                Storage {
                    times_wiped: 0,
                    storage: BTreeMap::from([(U256::from(1), U256::from(2))])
                }
            )]),
            "Should have changed 1 storage slot"
        );
        assert_eq!(post_state.bytecodes().len(), 0, "Should have zero new bytecodes");

        let accounts = post_state.accounts();
        assert_eq!(
            accounts.len(),
            3,
            "Should have 4 accounts (account 2, 3 and the ommer beneficiary)"
        );
        assert_eq!(
            accounts.get(&account2).unwrap(),
            &Some(account2_info_with_block_reward),
            "Account 2 state is wrong"
        );
        assert_eq!(
            accounts.get(&account3).unwrap(),
            &Some(account3_info),
            "Account 3 state is wrong"
        );
        assert_eq!(
            accounts.get(&ommer_beneficiary).unwrap(),
            &Some(ommer_beneficiary_info),
            "Ommer beneficiary state is wrong"
        );
    }
    */

    /* DAO HARDFORK change
    #[test]
    fn dao_hardfork_irregular_state_change() {
        let header = Header { number: 1, ..Header::default() };

        let mut db = StateProviderTest::default();

        let mut beneficiary_balance = 0;
        for (i, dao_address) in DAO_HARDKFORK_ACCOUNTS.iter().enumerate() {
            db.insert_account(
                *dao_address,
                Account { balance: U256::from(i), nonce: 0x00, bytecode_hash: None },
                None,
                HashMap::new(),
            );
            beneficiary_balance += i;
        }

        let chain_spec = Arc::new(
            ChainSpecBuilder::from(&*MAINNET)
                .homestead_activated()
                .with_fork(Hardfork::Dao, ForkCondition::Block(1))
                .build(),
        );

        let db = SubState::new(State::new(db));
        // execute chain and verify receipts
        let mut executor = Executor::new(chain_spec, db);
        let out = executor
            .execute_and_verify_receipt(
                &Block { header, body: vec![], ommers: vec![], withdrawals: None },
                U256::ZERO,
                None,
            )
            .unwrap();

        // Check if cache is set
        // beneficiary
        let db = executor.db();
        let dao_beneficiary = db.accounts.get(&DAO_HARDFORK_BENEFICIARY).unwrap();

        assert_eq!(dao_beneficiary.info.balance, U256::from(beneficiary_balance));
        for address in DAO_HARDKFORK_ACCOUNTS.iter() {
            let account = db.accounts.get(address).unwrap();
            assert_eq!(account.info.balance, U256::ZERO);
        }

        // check changesets
        let beneficiary_state = out.accounts().get(&DAO_HARDFORK_BENEFICIARY).unwrap().unwrap();
        assert_eq!(
            beneficiary_state,
            Account { balance: U256::from(beneficiary_balance), ..Default::default() },
        );
        for address in DAO_HARDKFORK_ACCOUNTS.iter() {
            let updated_account = out.accounts().get(address).unwrap().unwrap();
            assert_eq!(updated_account, Account { balance: U256::ZERO, ..Default::default() });
        }
    }
    */

    /* TEST Selfdestruct
    #[test]
    fn test_selfdestruct() {
        // Modified version of eth test. Storage is added for selfdestructed account to see
        // that changeset is set.
        // Got rlp block from: src/GeneralStateTestsFiller/stArgsZeroOneBalance/suicideNonConst.json

        let mut block_rlp = hex!("f9025ff901f7a0c86e8cc0310ae7c531c758678ddbfd16fc51c8cef8cec650b032de9869e8b94fa01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa050554882fbbda2c2fd93fdc466db9946ea262a67f7a76cc169e714f105ab583da00967f09ef1dfed20c0eacfaa94d5cd4002eda3242ac47eae68972d07b106d192a0e3c8b47fbfc94667ef4cceb17e5cc21e3b1eebd442cebb27f07562b33836290db90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008302000001830f42408238108203e800a00000000000000000000000000000000000000000000000000000000000000000880000000000000000f862f860800a83061a8094095e7baea6a6c7c4c2dfeb977efac326af552d8780801ba072ed817487b84ba367d15d2f039b5fc5f087d0a8882fbdf73e8cb49357e1ce30a0403d800545b8fc544f92ce8124e2255f8c3c6af93f28243a120585d4c4c6a2a3c0").as_slice();
        let block = Block::decode(&mut block_rlp).unwrap();
        let mut db = StateProviderTest::default();

        let address_caller = Address::from_str("a94f5374fce5edbc8e2a8697c15331677e6ebf0b").unwrap();
        let address_selfdestruct =
            Address::from_str("095e7baea6a6c7c4c2dfeb977efac326af552d87").unwrap();

        // pre state
        let pre_account_caller = Account {
            balance: U256::from(0x0de0b6b3a7640000u64),
            nonce: 0x00,
            bytecode_hash: None,
        };

        db.insert_account(address_caller, pre_account_caller, None, HashMap::new());

        // insert account that will selfd

        let pre_account_selfdestroyed = Account {
            balance: U256::ZERO,
            nonce: 0x00,
            bytecode_hash: Some(H256(hex!(
                "56a7d44a4ecf086c34482ad1feb1007087fc56fae6dbefbd3f416002933f1705"
            ))),
        };

        let selfdestroyed_storage =
            BTreeMap::from([(H256::zero(), U256::ZERO), (H256::from_low_u64_be(1), U256::from(1))]);
        db.insert_account(
            address_selfdestruct,
            pre_account_selfdestroyed,
            Some(hex!("73095e7baea6a6c7c4c2dfeb977efac326af552d8731ff00").into()),
            selfdestroyed_storage.into_iter().collect::<HashMap<_, _>>(),
        );

        // spec at berlin fork
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().berlin_activated().build());

        let db = SubState::new(State::new(db));

        // execute chain and verify receipts
        let mut executor = Executor::new(chain_spec, db);
        let out = executor.execute_and_verify_receipt(&block, U256::ZERO, None).unwrap();

        assert_eq!(out.bytecodes().len(), 0, "Should have zero new bytecodes");

        let post_account_caller = Account {
            balance: U256::from(0x0de0b6b3a761cf60u64),
            nonce: 0x01,
            bytecode_hash: None,
        };

        assert_eq!(
            out.accounts().get(&address_caller).unwrap().unwrap(),
            post_account_caller,
            "Caller account has changed and fee is deduced"
        );

        assert_eq!(
            out.accounts().get(&address_selfdestruct).unwrap(),
            &None,
            "Selfdestructed account should have been deleted"
        );
        assert!(
            out.storage().get(&address_selfdestruct).unwrap().wiped(),
            "Selfdestructed account should have its storage wiped"
        );
    }
    */

    /* ADD Withdrawal test
    // Test vector from https://github.com/ethereum/tests/blob/3156db5389921125bb9e04142d18e0e7b0cf8d64/BlockchainTests/EIPTests/bc4895-withdrawals/twoIdenticalIndexDifferentValidator.json
    #[test]
    fn test_withdrawals() {
        let block_rlp = hex!("f9028cf90219a0151934ad9b654c50197f37018ee5ee9bb922dec0a1b5e24a6d679cb111cdb107a01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa048cd9a5957e45beebf80278a5208b0cbe975ab4b4adb0da1509c67b26f2be3ffa056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008001887fffffffffffffff8082079e42a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b42188000000000000000009a04a220ebe55034d51f8a58175bb504b6ebf883105010a1f6d42e557c18bbd5d69c0c0f86cda808094c94f5374fce5edbc8e2a8697c15331677e6ebf0b822710da028094c94f5374fce5edbc8e2a8697c15331677e6ebf0b822710da018094c94f5374fce5edbc8e2a8697c15331677e6ebf0b822710da020194c94f5374fce5edbc8e2a8697c15331677e6ebf0b822710");
        let block = Block::decode(&mut block_rlp.as_slice()).unwrap();
        let withdrawals = block.withdrawals.as_ref().unwrap();
        assert_eq!(withdrawals.len(), 4);

        let withdrawal_beneficiary =
            Address::from_str("c94f5374fce5edbc8e2a8697c15331677e6ebf0b").unwrap();

        // spec at shanghai fork
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().shanghai_activated().build());

        let db = SubState::new(State::new(StateProviderTest::default()));

        // execute chain and verify receipts
        let mut executor = Executor::new(chain_spec, db);
        let out = executor.execute_and_verify_receipt(&block, U256::ZERO, None).unwrap();

        let withdrawal_sum = withdrawals.iter().fold(U256::ZERO, |sum, w| sum + w.amount_wei());
        let beneficiary_account = executor.db().accounts.get(&withdrawal_beneficiary).unwrap();
        assert_eq!(beneficiary_account.info.balance, withdrawal_sum);
        assert_eq!(beneficiary_account.info.nonce, 0);
        assert_eq!(beneficiary_account.account_state, AccountState::StorageCleared);

        assert_eq!(
            out.accounts().get(&withdrawal_beneficiary).unwrap(),
            &Some(Account { nonce: 0, balance: withdrawal_sum, bytecode_hash: None }),
            "Withdrawal account should have gotten its balance set"
        );

        // Execute same block again
        let out = executor.execute_and_verify_receipt(&block, U256::ZERO, None).unwrap();

        assert_eq!(
            out.accounts().get(&withdrawal_beneficiary).unwrap(),
            &Some(Account {
                nonce: 0,
                balance: withdrawal_sum + withdrawal_sum,
                bytecode_hash: None
            }),
            "Withdrawal account should have gotten its balance set"
        );
    }*/
}

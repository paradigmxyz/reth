//! Reuse of speculative prewarm execution results.
//!
//! Prewarm workers execute block transactions in parallel against the parent state and record
//! every account and storage value a transaction reads from the database. When sequential
//! execution reaches a transaction whose recorded reads still match the current state, it commits
//! the prewarm result instead of executing the transaction again. Missing or stale results fall
//! back to normal execution, which never waits for prewarming.
//!
//! Most conflicts between transactions are balance changes, such as fee credits to the block
//! beneficiary or value transfers to popular contracts. A transaction that never observes an
//! account's balance produces the same result for any balance, apart from the balance itself, so
//! such balances may differ and the transaction's balance change is applied to the current balance
//! instead. Balances are observed by `BALANCE`, `SELFBALANCE`, `SELFDESTRUCT` and failed value
//! transfers. Successful value transfers and gas payments only require the balance to be at least
//! as large as during speculation.

use alloy_primitives::{Address, B256, U256};
use parking_lot::Mutex;
use revm::{
    bytecode::opcode,
    context::result::ResultAndState,
    interpreter::{
        instructions::utility::IntoAddress, interpreter_types::Jumps, CallInputs, CallOutcome,
        CreateInputs, CreateOutcome, InstructionResult, Interpreter,
    },
    primitives::{StorageKey, StorageValue},
    state::{AccountInfo, Bytecode},
    Database, Inspector,
};

/// Balances above this are compared exactly, so that applying balance changes cannot overflow.
const MAX_REBASED_BALANCE: U256 = U256::from_limbs([u64::MAX, u64::MAX, 0, 0]);

/// Speculative prewarm results of a block's transactions, indexed by their position in the block.
#[derive(Debug)]
pub struct PrewarmResults<H> {
    slots: Box<[ResultSlot<H>]>,
}

/// Slot of a [`PrewarmResults`] transaction, empty until prewarming finishes it.
type ResultSlot<H> = Mutex<Option<Box<PrewarmedTx<H>>>>;

impl<H> PrewarmResults<H> {
    /// Creates empty result slots for `transaction_count` transactions.
    pub fn new(transaction_count: usize) -> Self {
        Self { slots: (0..transaction_count).map(|_| Mutex::new(None)).collect() }
    }

    /// Stores the prewarm result of the transaction at `index`.
    pub(super) fn insert(&self, index: usize, result: ResultAndState<H>, reads: ReadSet) {
        if let Some(slot) = self.slots.get(index) {
            *slot.lock() = Some(Box::new(PrewarmedTx { result, reads }));
        }
    }

    /// Takes the prewarm result of the transaction at `index` if it is ready and its reads match
    /// `db`.
    ///
    /// The returned result has its balance changes applied to the current balances. Stale results
    /// stay in their slot and are freed with `self`.
    pub fn take_valid<DB: Database>(
        &self,
        index: usize,
        db: &mut DB,
    ) -> Result<ResultAndState<H>, ReuseMiss> {
        let mut slot = self.slots.get(index).ok_or(ReuseMiss::Missing)?.lock();
        let tx = slot.as_mut().ok_or(ReuseMiss::Missing)?;
        if !tx.validate(db) {
            return Err(ReuseMiss::Stale)
        }
        Ok(slot.take().expect("validated result").result)
    }
}

/// Reason a prewarm result could not be reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReuseMiss {
    /// Prewarming had not finished the transaction.
    Missing,
    /// The transaction read state that changed before it in the block.
    Stale,
}

/// A speculative transaction result and the database reads it depends on.
#[derive(Debug)]
struct PrewarmedTx<H> {
    result: ResultAndState<H>,
    reads: ReadSet,
}

impl<H> PrewarmedTx<H> {
    /// Returns whether all reads match `db`, applying the result's balance changes to the current
    /// balances of accounts whose balance differs.
    ///
    /// The result must not be used if this returns `false`.
    fn validate<DB: Database>(&mut self, db: &mut DB) -> bool {
        for read in &self.reads.accounts {
            let Ok(current) = db.basic(read.address) else { return false };
            let (speculative, current) = match (&read.info, current) {
                (Some(speculative), Some(current)) => (speculative, current),
                (None, None) => continue,
                _ => return false,
            };
            let balance = current.balance;
            if (AccountInfo { balance: speculative.balance, ..current }) != *speculative {
                return false
            }
            if balance == speculative.balance {
                continue
            }

            let allowed = match read.balance {
                BalanceDependency::None => true,
                BalanceDependency::AtLeast => balance > speculative.balance,
                BalanceDependency::Exact => false,
            };
            // A zero balance can make the account empty, which changes gas costs and EIP-161
            // state clearing.
            if !allowed ||
                speculative.balance.is_zero() ||
                balance.is_zero() ||
                balance > MAX_REBASED_BALANCE
            {
                return false
            }
            if let Some(account) = self.result.state.get_mut(&read.address) {
                // All balances are bounded, so this cannot overflow.
                account.info.balance = account.info.balance + balance - speculative.balance;
            }
        }
        self.reads
            .storage
            .iter()
            .all(|(address, slot, expected)| db.storage(*address, *slot).ok() == Some(*expected))
    }
}

/// How a speculative execution depends on an account's balance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BalanceDependency {
    /// The balance was only credited or debited, so it may differ.
    None,
    /// The account paid for gas or sent value, so the balance must not be lower.
    AtLeast,
    /// The balance was observed, so it must match.
    Exact,
}

/// An account a speculative execution read from the database.
#[derive(Debug)]
struct AccountRead {
    address: Address,
    /// The account without its bytecode, `None` if it did not exist.
    info: Option<AccountInfo>,
    balance: BalanceDependency,
}

/// Database reads of a speculative transaction execution.
#[derive(Debug, Default)]
pub(super) struct ReadSet {
    accounts: Vec<AccountRead>,
    storage: Vec<(Address, StorageKey, StorageValue)>,
}

impl ReadSet {
    /// Classifies how the execution depends on the balance of each account it read.
    ///
    /// Speculation disables nonce and balance checks, so this returns `None` unless `caller` had
    /// the transaction's `nonce` and could pay `max_balance_spending`, i.e. unless execution with
    /// these checks enabled would have accepted the transaction.
    pub(super) fn finish(
        mut self,
        balances: &BalanceTracker,
        caller: Address,
        nonce: u64,
        max_balance_spending: U256,
    ) -> Option<Self> {
        // Balance changes are applied once per account.
        self.accounts.sort_unstable_by_key(|read| read.address);
        self.accounts.dedup_by_key(|read| read.address);

        let caller_index = self.accounts.binary_search_by_key(&caller, |read| read.address).ok()?;
        let caller_info = &self.accounts[caller_index].info;
        let (caller_nonce, caller_balance) =
            caller_info.as_ref().map_or((0, U256::ZERO), |info| (info.nonce, info.balance));
        if caller_nonce != nonce || nonce == u64::MAX || caller_balance < max_balance_spending {
            return None
        }

        // Rebasing only bounded balances keeps every balance during execution far from overflow,
        // so no value transfer can overflow in one execution but not the other.
        let bounded = self
            .accounts
            .iter()
            .all(|read| read.info.as_ref().is_none_or(|info| info.balance <= MAX_REBASED_BALANCE));
        for read in &mut self.accounts {
            read.balance = if !bounded || balances.observed.contains(&read.address) {
                BalanceDependency::Exact
            } else if read.address == caller || balances.senders.contains(&read.address) {
                BalanceDependency::AtLeast
            } else {
                BalanceDependency::None
            };
        }
        Some(self)
    }
}

/// Database wrapper that records reads for [`PrewarmResults`].
#[derive(Debug)]
pub(super) struct RecordingDb<DB> {
    inner: DB,
    /// Reads of the current transaction, `None` if recording is disabled.
    reads: Option<ReadSet>,
}

impl<DB> RecordingDb<DB> {
    /// Wraps `inner` without recording.
    pub(super) const fn new(inner: DB) -> Self {
        Self { inner, reads: None }
    }

    /// Starts recording the reads of a new transaction.
    pub(super) fn start_recording(&mut self) {
        self.reads = Some(ReadSet::default());
    }

    /// Stops recording and returns the reads of the current transaction.
    pub(super) const fn take_reads(&mut self) -> Option<ReadSet> {
        self.reads.take()
    }
}

impl<DB: Database> Database for RecordingDb<DB> {
    type Error = DB::Error;

    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        let account = self.inner.basic(address)?;
        if let Some(reads) = &mut self.reads {
            reads.accounts.push(AccountRead {
                address,
                info: account.as_ref().map(AccountInfo::copy_without_code),
                balance: BalanceDependency::None,
            });
        }
        Ok(account)
    }

    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        self.inner.code_by_hash(code_hash)
    }

    fn storage(
        &mut self,
        address: Address,
        index: StorageKey,
    ) -> Result<StorageValue, Self::Error> {
        let value = self.inner.storage(address, index)?;
        if let Some(reads) = &mut self.reads {
            reads.storage.push((address, index, value));
        }
        Ok(value)
    }

    fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
        self.inner.block_hash(number)
    }
}

/// Inspector that tracks which balances a transaction observes and which accounts send value.
#[derive(Debug, Default)]
pub(super) struct BalanceTracker {
    /// Accounts whose balance was observed.
    observed: Vec<Address>,
    /// Accounts that sent value.
    senders: Vec<Address>,
}

impl BalanceTracker {
    /// Clears the tracked accounts for a new transaction.
    pub(super) fn clear(&mut self) {
        self.observed.clear();
        self.senders.clear();
    }
}

impl<CTX> Inspector<CTX> for BalanceTracker {
    fn step(&mut self, interp: &mut Interpreter, _context: &mut CTX) {
        let address = match interp.bytecode.opcode() {
            opcode::BALANCE => {
                let Ok(word) = interp.stack.peek(0) else { return };
                word.into_address()
            }
            opcode::SELFBALANCE | opcode::SELFDESTRUCT => interp.input.target_address,
            _ => return,
        };
        // Skip repeated observations, such as in a loop.
        if self.observed.last() != Some(&address) {
            self.observed.push(address);
        }
    }

    fn call(&mut self, _context: &mut CTX, inputs: &mut CallInputs) -> Option<CallOutcome> {
        if inputs.transfers_value() {
            self.senders.push(inputs.transfer_from());
        }
        None
    }

    fn call_end(&mut self, _context: &mut CTX, inputs: &CallInputs, outcome: &mut CallOutcome) {
        if *outcome.instruction_result() == InstructionResult::OutOfFunds {
            self.observed.push(inputs.transfer_from());
        }
    }

    fn create(&mut self, _context: &mut CTX, inputs: &mut CreateInputs) -> Option<CreateOutcome> {
        if !inputs.value().is_zero() {
            self.senders.push(inputs.caller());
        }
        None
    }

    fn create_end(
        &mut self,
        _context: &mut CTX,
        inputs: &CreateInputs,
        outcome: &mut CreateOutcome,
    ) {
        if *outcome.instruction_result() == InstructionResult::OutOfFunds {
            self.observed.push(inputs.caller());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{transaction::Recovered, TxLegacy};
    use alloy_evm::{block::BlockExecutor, eth::EthBlockExecutionCtx, Evm, EvmEnv};
    use alloy_primitives::{bytes, Bytes, Signature, TxKind};
    use reth_ethereum_primitives::{Receipt, Transaction, TransactionSigned};
    use reth_evm::{ConfigureEvm, HaltReasonFor};
    use reth_evm_ethereum::EthEvmConfig;
    use revm::{
        context::{BlockEnv, CfgEnv, Transaction as _},
        database::{states::bundle_state::BundleRetention, BundleState, CacheDB, EmptyDB, State},
        primitives::hardfork::SpecId,
    };

    const BENEFICIARY: Address = Address::repeat_byte(0xbe);
    /// Does nothing.
    const NOOP: Address = Address::repeat_byte(0xc0);
    /// Increments slot 0.
    const COUNTER: Address = Address::repeat_byte(0xc1);
    /// Stores the beneficiary balance in slot 0.
    const BENEFICIARY_BALANCE_READER: Address = Address::repeat_byte(0xc2);
    /// Stores its own balance in slot 0.
    const SELF_BALANCE_READER: Address = Address::repeat_byte(0xc3);
    /// Sends 1 wei to [`NOOP`].
    const FORWARDER: Address = Address::repeat_byte(0xc4);
    const GAS_PRICE: u128 = 2;

    type Results = PrewarmResults<HaltReasonFor<EthEvmConfig>>;

    fn evm_env(prewarm: bool) -> EvmEnv {
        let mut cfg_env = CfgEnv::new_with_spec(SpecId::CANCUN).with_chain_id(1);
        cfg_env.disable_nonce_check = prewarm;
        cfg_env.disable_balance_check = prewarm;
        let block_env = BlockEnv {
            number: U256::from(1),
            beneficiary: BENEFICIARY,
            basefee: 1,
            gas_limit: 30_000_000,
            ..Default::default()
        };
        EvmEnv { cfg_env, block_env }
    }

    fn sender(i: u8) -> Address {
        Address::repeat_byte(i)
    }

    fn tx(from: u8, nonce: u64, to: Address, value: u64) -> Recovered<TransactionSigned> {
        let tx = Transaction::Legacy(TxLegacy {
            chain_id: Some(1),
            nonce,
            gas_price: GAS_PRICE,
            gas_limit: 100_000,
            to: TxKind::Call(to),
            value: U256::from(value),
            input: Bytes::new(),
        });
        Recovered::new_unchecked(
            TransactionSigned::new_unhashed(tx, Signature::test_signature()),
            sender(from),
        )
    }

    fn parent_state() -> CacheDB<EmptyDB> {
        let mut db = CacheDB::new(EmptyDB::default());
        for i in 1..=4 {
            db.insert_account_info(
                sender(i),
                AccountInfo { balance: U256::from(10u64.pow(18)), ..Default::default() },
            );
        }
        db.insert_account_info(
            BENEFICIARY,
            AccountInfo { balance: U256::from(1), ..Default::default() },
        );
        for (address, code) in [
            (NOOP, bytes!("00")),
            (COUNTER, bytes!("5f546001015f5500")),
            (BENEFICIARY_BALANCE_READER, bytes!("41315f5500")),
            (SELF_BALANCE_READER, bytes!("475f5500")),
            (FORWARDER, bytes!("5f5f5f5f600173c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c05af100")),
        ] {
            let code = Bytecode::new_raw(code);
            db.insert_account_info(
                address,
                AccountInfo {
                    balance: U256::from(10),
                    code_hash: code.hash_slow(),
                    code: Some(code),
                    ..Default::default()
                },
            );
        }
        db
    }

    /// Executes `txs` in parallel-prewarm fashion, each against the parent state.
    fn prewarm(txs: &[Recovered<TransactionSigned>]) -> Results {
        let config = EthEvmConfig::mainnet();
        let results = Results::new(txs.len());
        let mut evm = config.evm_with_env_and_inspector(
            RecordingDb::new(parent_state()),
            evm_env(true),
            BalanceTracker::default(),
        );
        for (index, tx) in txs.iter().enumerate() {
            let tx_env = config.tx_env(tx);
            let (nonce, max_balance_spending) =
                (tx_env.nonce, tx_env.max_balance_spending().unwrap());
            evm.db_mut().start_recording();
            evm.inspector_mut().clear();
            let result = evm.transact(tx_env).unwrap();
            let reads = evm.db_mut().take_reads().unwrap();
            if let Some(reads) =
                reads.finish(evm.inspector(), tx.signer(), nonce, max_balance_spending)
            {
                results.insert(index, result, reads);
            }
        }
        results
    }

    /// Executes `txs` sequentially, committing valid prewarm results instead of executing.
    fn execute(
        txs: &[Recovered<TransactionSigned>],
        results: Option<&Results>,
    ) -> (Vec<Receipt>, BundleState, Vec<Result<(), ReuseMiss>>) {
        let config = EthEvmConfig::mainnet();
        let mut state = State::builder().with_database(parent_state()).with_bundle_update().build();
        let evm = config.evm_with_env(&mut state, evm_env(false));
        let ctx = EthBlockExecutionCtx {
            parent_hash: B256::ZERO,
            parent_beacon_block_root: None,
            ommers: &[],
            withdrawals: None,
            extra_data: Bytes::new(),
            tx_count_hint: None,
            slot_number: None,
        };
        let mut executor = config.create_executor(evm, ctx);
        let mut outcomes = Vec::new();
        for (index, tx) in txs.iter().enumerate() {
            let outcome = results.map_or(Err(ReuseMiss::Missing), |results| {
                let result = results.take_valid(index, executor.evm_mut().db_mut())?;
                executor.commit_transaction(config.prewarm_tx_result(tx.inner(), result).unwrap());
                Ok(())
            });
            if outcome.is_err() {
                executor.execute_transaction(tx).unwrap();
            }
            outcomes.push(outcome);
        }
        let receipts = executor.receipts().to_vec();
        drop(executor);
        state.merge_transitions(BundleRetention::Reverts);
        (receipts, state.take_bundle(), outcomes)
    }

    /// Asserts that reusing prewarm results produces the same output as executing every
    /// transaction, and returns the reuse outcomes.
    fn reuse(txs: &[Recovered<TransactionSigned>]) -> Vec<Result<(), ReuseMiss>> {
        let (receipts, bundle, outcomes) = execute(txs, Some(&prewarm(txs)));
        let (expected_receipts, expected_bundle, _) = execute(txs, None);
        assert_eq!(receipts, expected_receipts);
        assert_eq!(bundle, expected_bundle);
        outcomes
    }

    #[test]
    fn reuses_independent_transactions() {
        let txs = [tx(1, 0, sender(2), 1), tx(3, 0, Address::repeat_byte(0xaa), 1)];
        assert_eq!(reuse(&txs), [Ok(()), Ok(())]);
    }

    #[test]
    fn rebases_unobserved_balances() {
        // The second transaction's caller and recipient were credited by the first.
        let txs = [tx(1, 0, NOOP, 1), tx(1, 1, sender(2), 1), tx(2, 0, NOOP, 1)];
        assert_eq!(reuse(&txs), [Ok(()), Err(ReuseMiss::Missing), Ok(())]);
    }

    #[test]
    fn rejects_conflicting_storage() {
        let txs = [tx(1, 0, COUNTER, 1), tx(2, 0, COUNTER, 1)];
        assert_eq!(reuse(&txs), [Ok(()), Err(ReuseMiss::Stale)]);
    }

    #[test]
    fn rejects_observed_balances() {
        let txs = [
            tx(1, 0, SELF_BALANCE_READER, 1),
            tx(2, 0, SELF_BALANCE_READER, 1),
            tx(3, 0, BENEFICIARY_BALANCE_READER, 1),
        ];
        assert_eq!(reuse(&txs), [Ok(()), Err(ReuseMiss::Stale), Err(ReuseMiss::Stale)]);
    }

    #[test]
    fn rejects_lower_sender_balances() {
        let txs = [tx(1, 0, FORWARDER, 0), tx(2, 0, FORWARDER, 0), tx(3, 0, FORWARDER, 2)];
        assert_eq!(reuse(&txs), [Ok(()), Err(ReuseMiss::Stale), Err(ReuseMiss::Stale)]);
    }
}

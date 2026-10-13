//! Reuse of speculative prewarm execution results.
//!
//! Prewarm workers execute block transactions in parallel against the parent state and record
//! every account and storage value a transaction reads from the database. When sequential
//! execution reaches a transaction whose recorded reads still match the current state, it commits
//! the prewarm result instead of executing the transaction again. Missing or stale results fall
//! back to normal execution, which never waits for prewarming.
//!
//! Nearly every transaction credits fees to the block beneficiary, so requiring its balance to
//! match would reject most results. If a transaction first loaded the beneficiary after its
//! outermost call frame ended, only the fee credit touched the account, and the credit is added
//! to the current beneficiary balance instead.

use alloy_primitives::{Address, B256, U256};
use parking_lot::Mutex;
use revm::{
    context::result::ResultAndState,
    handler::FrameResult,
    interpreter::{FrameInput, InterpreterTypes},
    primitives::{StorageKey, StorageValue},
    state::{AccountInfo, Bytecode},
    Database, Inspector,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

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
    /// The returned result has its beneficiary fee credit applied to the current beneficiary
    /// balance. Stale results stay in their slot and are freed with `self`.
    pub fn take_valid<DB: Database>(
        &self,
        index: usize,
        db: &mut DB,
        beneficiary: Address,
    ) -> Result<ResultAndState<H>, ReuseMiss> {
        let mut slot = self.slots.get(index).ok_or(ReuseMiss::Missing)?.lock();
        let rebased_beneficiary = slot
            .as_ref()
            .ok_or(ReuseMiss::Missing)?
            .validate(db, beneficiary)
            .ok_or(ReuseMiss::Stale)?;
        let mut result = slot.take().ok_or(ReuseMiss::Missing)?.result;
        if let Some(info) = rebased_beneficiary &&
            let Some(account) = result.state.get_mut(&beneficiary)
        {
            account.info = info;
        }
        Ok(result)
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
    /// Returns whether all reads match `db`, with the beneficiary account to commit if its fee
    /// credit must be applied to its current balance.
    fn validate<DB: Database>(
        &self,
        db: &mut DB,
        beneficiary: Address,
    ) -> Option<Option<AccountInfo>> {
        for (address, expected) in &self.reads.accounts {
            if db.basic(*address).ok()? != *expected {
                return None
            }
        }
        for (address, slot, expected) in &self.reads.storage {
            if db.storage(*address, *slot).ok()? != *expected {
                return None
            }
        }

        let Some((expected, observed)) = &self.reads.beneficiary else { return Some(None) };
        let current = db.basic(beneficiary).ok()?;
        if *observed {
            return (current == *expected).then_some(None)
        }

        // Only the fee credit touched the beneficiary. Both states must be non-empty, so touching
        // the account cannot clear it in one execution but not the other.
        let (Some(expected), Some(mut current)) = (expected, current) else { return None };
        if expected.is_empty() || current.is_empty() {
            return None
        }
        let credit =
            self.result.state.get(&beneficiary)?.info.balance.checked_sub(expected.balance)?;
        current.balance = current.balance.checked_add(credit)?;
        Some(Some(current))
    }
}

/// Database reads of a speculative transaction execution.
#[derive(Debug, Default)]
pub(super) struct ReadSet {
    /// Accounts other than the beneficiary, without their bytecode.
    accounts: Vec<(Address, Option<AccountInfo>)>,
    /// Storage slots.
    storage: Vec<(Address, StorageKey, StorageValue)>,
    /// The beneficiary, and whether it was loaded before the outermost call frame ended.
    beneficiary: Option<(Option<AccountInfo>, bool)>,
}

impl ReadSet {
    /// Returns whether `caller` had the given nonce and could pay `max_balance_spending`, i.e.
    /// whether execution with nonce and balance checks enabled would have accepted the
    /// transaction.
    pub(super) fn caller_is_valid(
        &self,
        caller: Address,
        nonce: u64,
        max_balance_spending: U256,
    ) -> bool {
        let Some(account) = self
            .accounts
            .iter()
            .find_map(|(address, account)| (*address == caller).then_some(account))
        else {
            return false
        };
        let (account_nonce, balance) =
            account.as_ref().map_or((0, U256::ZERO), |account| (account.nonce, account.balance));
        account_nonce == nonce && nonce != u64::MAX && balance >= max_balance_spending
    }
}

/// Database wrapper that records reads for [`PrewarmResults`].
#[derive(Debug)]
pub(super) struct RecordingDb<DB> {
    inner: DB,
    beneficiary: Address,
    /// Set by [`FrameTracker`] when the outermost call frame ends.
    frames_done: Arc<AtomicBool>,
    /// Reads of the current transaction, `None` if recording is disabled.
    reads: Option<ReadSet>,
}

impl<DB> RecordingDb<DB> {
    /// Wraps `inner`, sharing `frames_done` with the [`FrameTracker`] of the same EVM.
    pub(super) const fn new(inner: DB, beneficiary: Address, frames_done: Arc<AtomicBool>) -> Self {
        Self { inner, beneficiary, frames_done, reads: None }
    }

    /// Starts recording the reads of a new transaction.
    pub(super) fn start_recording(&mut self) {
        self.frames_done.store(false, Ordering::Relaxed);
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
            let read = account.as_ref().map(AccountInfo::copy_without_code);
            if address == self.beneficiary {
                reads.beneficiary = Some((read, !self.frames_done.load(Ordering::Relaxed)));
            } else {
                reads.accounts.push((address, read));
            }
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

/// Inspector that signals [`RecordingDb`] when a transaction's outermost call frame ends.
#[derive(Debug)]
pub(super) struct FrameTracker {
    depth: usize,
    frames_done: Arc<AtomicBool>,
}

impl FrameTracker {
    /// Creates a tracker that sets `frames_done` when the outermost frame ends.
    pub(super) const fn new(frames_done: Arc<AtomicBool>) -> Self {
        Self { depth: 0, frames_done }
    }
}

impl<CTX, INTR: InterpreterTypes> Inspector<CTX, INTR> for FrameTracker {
    fn frame_start(&mut self, _context: &mut CTX, _input: &mut FrameInput) -> Option<FrameResult> {
        self.depth += 1;
        None
    }

    fn frame_end(&mut self, _context: &mut CTX, _input: &FrameInput, _result: &mut FrameResult) {
        self.depth = self.depth.saturating_sub(1);
        if self.depth == 0 {
            self.frames_done.store(true, Ordering::Relaxed);
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
        state::Bytecode,
    };

    const BENEFICIARY: Address = Address::repeat_byte(0xbe);
    const COUNTER: Address = Address::repeat_byte(0xc0);
    const BENEFICIARY_BALANCE_READER: Address = Address::repeat_byte(0xc1);
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

    fn tx(from: u8, nonce: u64, to: Address) -> Recovered<TransactionSigned> {
        let tx = Transaction::Legacy(TxLegacy {
            chain_id: Some(1),
            nonce,
            gas_price: GAS_PRICE,
            gas_limit: 100_000,
            to: TxKind::Call(to),
            value: U256::from(1),
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
        // Increments slot 0.
        let counter = Bytecode::new_raw(bytes!("5f546001015f5500"));
        // Stores the beneficiary balance in slot 0.
        let balance_reader = Bytecode::new_raw(bytes!("41315f5500"));
        for (address, code) in [(COUNTER, counter), (BENEFICIARY_BALANCE_READER, balance_reader)] {
            db.insert_account_info(
                address,
                AccountInfo { code_hash: code.hash_slow(), code: Some(code), ..Default::default() },
            );
        }
        db
    }

    /// Executes `txs` in parallel-prewarm fashion, each against the parent state.
    fn prewarm(txs: &[Recovered<TransactionSigned>]) -> Results {
        let config = EthEvmConfig::mainnet();
        let results = Results::new(txs.len());
        let frames_done = Arc::new(AtomicBool::new(false));
        let db = RecordingDb::new(parent_state(), BENEFICIARY, Arc::clone(&frames_done));
        let mut evm =
            config.evm_with_env_and_inspector(db, evm_env(true), FrameTracker::new(frames_done));
        for (index, tx) in txs.iter().enumerate() {
            let tx_env = config.tx_env(tx);
            let (nonce, max_balance_spending) =
                (tx_env.nonce, tx_env.max_balance_spending().unwrap());
            evm.db_mut().start_recording();
            let result = evm.transact(tx_env).unwrap();
            let reads = evm.db_mut().take_reads().unwrap();
            if reads.caller_is_valid(tx.signer(), nonce, max_balance_spending) {
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
                let result = results.take_valid(index, executor.evm_mut().db_mut(), BENEFICIARY)?;
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
        let txs = [tx(1, 0, sender(2)), tx(3, 0, Address::repeat_byte(0xaa))];
        assert_eq!(reuse(&txs), [Ok(()), Ok(())]);
    }

    #[test]
    fn rejects_conflicting_storage() {
        let txs = [tx(1, 0, COUNTER), tx(2, 0, COUNTER)];
        assert_eq!(reuse(&txs), [Ok(()), Err(ReuseMiss::Stale)]);
    }

    #[test]
    fn rejects_transactions_after_caller_changes() {
        let txs = [tx(1, 0, sender(2)), tx(2, 0, sender(3)), tx(1, 1, sender(3))];
        assert_eq!(reuse(&txs), [Ok(()), Err(ReuseMiss::Stale), Err(ReuseMiss::Missing)]);
    }

    #[test]
    fn rejects_observed_beneficiary_balance() {
        let txs = [tx(1, 0, sender(2)), tx(3, 0, BENEFICIARY_BALANCE_READER)];
        assert_eq!(reuse(&txs), [Ok(()), Err(ReuseMiss::Stale)]);
    }
}

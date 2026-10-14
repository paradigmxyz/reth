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

use alloy_primitives::{map::DefaultHashBuilder, Address, B256, U256};
use dashmap::DashMap;
use parking_lot::Mutex;
use revm::{
    bytecode::opcode,
    context::{result::ResultAndState, ContextTr, JournalTr},
    interpreter::{
        instructions::utility::IntoAddress, interpreter_types::Jumps, CallInputs, CallOutcome,
        CreateInputs, CreateOutcome, InstructionResult, Interpreter,
    },
    primitives::{StorageKey, StorageValue, KECCAK_EMPTY},
    state::{Account, AccountInfo, Bytecode, EvmState},
    Database, Inspector,
};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

/// Balances above this are compared exactly, so that applying balance changes cannot overflow.
const MAX_REBASED_BALANCE: U256 = U256::from_limbs([u64::MAX, u64::MAX, 0, 0]);

/// Speculative prewarm results of a block's transactions, indexed by their position in the block.
#[derive(Debug)]
pub struct PrewarmResults<H> {
    slots: Box<[Mutex<ResultSlot<H>>]>,
    /// Writes predicted by the stored results.
    predicted: Arc<PredictedState>,
}

/// Prewarm state of a transaction in [`PrewarmResults`].
#[derive(Debug)]
enum ResultSlot<H> {
    /// Prewarming has not executed the transaction.
    Empty,
    /// A result that may be reused.
    Ready(Box<PrewarmedTx<H>>),
    /// A result that cannot be reused, with the reads it depends on.
    Unusable(ReadSet),
    /// Block execution executed the transaction or reused its result, which are published as
    /// its actual writes.
    Executed,
}

impl<H> PrewarmResults<H> {
    /// Creates empty result slots for `transaction_count` transactions.
    pub fn new(transaction_count: usize) -> Self {
        Self {
            slots: (0..transaction_count).map(|_| Mutex::new(ResultSlot::Empty)).collect(),
            predicted: Arc::default(),
        }
    }

    /// Returns the writes predicted by the stored results.
    pub(super) const fn predicted(&self) -> &Arc<PredictedState> {
        &self.predicted
    }

    /// Stores the prewarm result of the transaction at `index` and publishes its writes.
    pub(super) fn insert(&self, index: usize, result: ResultAndState<H>, reads: ReadSet) {
        self.set(index, |predicted| {
            predicted.publish(index, &result.state);
            ResultSlot::Ready(Box::new(PrewarmedTx { result, reads }))
        });
    }

    /// Marks the result of the transaction at `index` as unusable.
    pub(super) fn reject(&self, index: usize, mut reads: ReadSet) {
        // Rejection may depend on any balance, such as the caller's.
        for read in &mut reads.accounts {
            read.balance = BalanceDependency::Exact;
        }
        self.set(index, |_| ResultSlot::Unusable(reads));
    }

    /// Replaces the predicted writes of the transaction at `index` with the writes of its actual
    /// execution by block execution.
    pub fn publish_executed(&self, index: usize, state: &EvmState) {
        let Some(slot) = self.slots.get(index) else { return };
        let mut slot = slot.lock();
        // Overwrite the predicted writes in place, then retract those the execution did not make.
        self.predicted.publish(index, state);
        if let ResultSlot::Ready(previous) = core::mem::replace(&mut *slot, ResultSlot::Executed) {
            self.predicted.retract_unwritten(index, &previous.result.state, state);
        }
        self.predicted.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Replaces the slot of the transaction at `index`, retracting the writes of its previous
    /// result, unless block execution already published its actual writes.
    fn set(&self, index: usize, f: impl FnOnce(&PredictedState) -> ResultSlot<H>) {
        let Some(slot) = self.slots.get(index) else { return };
        let mut slot = slot.lock();
        match &*slot {
            ResultSlot::Ready(previous) => self.predicted.retract(index, &previous.result.state),
            ResultSlot::Executed => return,
            ResultSlot::Empty | ResultSlot::Unusable(_) => {}
        }
        *slot = f(&self.predicted);
    }

    /// Returns whether the transaction at `index` should be executed again, because prewarming
    /// skipped it or its reads no longer match the predicted state.
    pub(super) fn needs_refresh(&self, index: usize) -> bool {
        let Some(slot) = self.slots.get(index) else { return false };
        let slot = slot.lock();
        let reads = match &*slot {
            ResultSlot::Empty => return true,
            ResultSlot::Ready(tx) => &tx.reads,
            ResultSlot::Unusable(reads) => reads,
            ResultSlot::Executed => return false,
        };
        !reads.matches(&self.predicted, index)
    }

    /// Returns whether prewarming executed the transaction at `index`, but its reads no longer
    /// match the predicted state.
    pub(super) fn is_stale(&self, index: usize) -> bool {
        let Some(slot) = self.slots.get(index) else { return false };
        let slot = slot.lock();
        let reads = match &*slot {
            ResultSlot::Ready(tx) => &tx.reads,
            ResultSlot::Unusable(reads) => reads,
            ResultSlot::Empty | ResultSlot::Executed => return false,
        };
        !reads.matches(&self.predicted, index)
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
        let tx = match &mut *slot {
            ResultSlot::Empty | ResultSlot::Executed => return Err(ReuseMiss::Missing),
            ResultSlot::Ready(tx) => tx,
            ResultSlot::Unusable(_) => return Err(ReuseMiss::Stale),
        };
        let Some(rebased) = tx.validate(db) else { return Err(ReuseMiss::Stale) };
        if rebased {
            // Later transactions read the rebased balances.
            self.predicted.publish_accounts(index, &tx.result.state);
        }
        // The result stays published as the prediction of this transaction's writes.
        let ResultSlot::Ready(tx) = core::mem::replace(&mut *slot, ResultSlot::Executed) else {
            unreachable!("validated result")
        };
        Ok(tx.result)
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
    /// balances of accounts whose balance differs, and whether there were any.
    ///
    /// The result must not be used if this returns `None`.
    fn validate<DB: Database>(&mut self, db: &mut DB) -> Option<bool> {
        let mut rebased = false;
        for read in &self.reads.accounts {
            let current = db.basic(read.address).ok()?;
            if let Some((speculative, current)) = read.compare(current.as_ref())? &&
                let Some(account) = self.result.state.get_mut(&read.address)
            {
                // All balances are bounded, and the execution never leaves less than the
                // difference, so this neither overflows nor underflows.
                account.info.balance =
                    account.info.balance.checked_add(current)?.checked_sub(speculative)?;
                rebased = true;
            }
        }
        self.reads
            .storage
            .iter()
            .all(|read| db.storage(read.address, read.slot).ok() == Some(read.value))
            .then_some(rebased)
    }
}

/// How a speculative execution depends on an account's balance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BalanceDependency {
    /// The balance was only credited or debited, so it may differ.
    None,
    /// The account paid for gas or sent value, so the balance must be at least this large.
    AtLeast(U256),
    /// The balance was observed, so it must match.
    Exact,
}

/// An account a speculative execution read.
#[derive(Debug)]
struct AccountRead {
    address: Address,
    /// The account without its bytecode, `None` if it did not exist.
    info: Option<AccountInfo>,
    balance: BalanceDependency,
    /// Whether the account was read from the predicted state.
    predicted: bool,
}

impl AccountRead {
    /// Compares the read account with `current`.
    ///
    /// Returns `None` if they differ in a way the execution may depend on, otherwise the
    /// `(speculative, current)` balances if the result's balance change must be applied to the
    /// current balance.
    fn compare(&self, current: Option<&AccountInfo>) -> Option<Option<(U256, U256)>> {
        let (speculative, current) = match (&self.info, current) {
            (Some(speculative), Some(current)) => (speculative, current),
            (None, None) => return Some(None),
            _ => return None,
        };
        if speculative.balance == current.balance {
            return (speculative == current).then_some(None)
        }

        let allowed = match self.balance {
            BalanceDependency::None => true,
            BalanceDependency::AtLeast(min) => current.balance >= min,
            BalanceDependency::Exact => false,
        };
        // A zero balance can make the account empty, which changes gas costs and EIP-161 state
        // clearing.
        if !allowed ||
            speculative.balance.is_zero() ||
            current.balance.is_zero() ||
            current.balance > MAX_REBASED_BALANCE ||
            (AccountInfo { balance: speculative.balance, ..current.copy_without_code() }) !=
                *speculative
        {
            return None
        }
        Some(Some((speculative.balance, current.balance)))
    }
}

/// A storage slot a speculative execution read.
#[derive(Debug)]
struct StorageRead {
    address: Address,
    slot: StorageKey,
    value: StorageValue,
    /// Whether the slot was read from the predicted state.
    predicted: bool,
}

/// Database reads of a speculative transaction execution.
#[derive(Debug, Default)]
pub(super) struct ReadSet {
    accounts: Vec<AccountRead>,
    storage: Vec<StorageRead>,
    /// Whether execution loaded bytecode that is unknown to the database.
    unknown_code: bool,
}

impl ReadSet {
    /// Classifies how the execution depends on the balance of each account it read.
    ///
    /// Speculation disables nonce and balance checks, so this fails unless `caller` had the
    /// transaction's `nonce` and could pay `max_balance_spending`, i.e. unless execution with
    /// these checks enabled would have accepted the transaction.
    pub(super) fn finish(
        mut self,
        balances: &BalanceTracker,
        caller: Address,
        nonce: u64,
        max_balance_spending: Option<U256>,
    ) -> Result<Self, Self> {
        // Balance changes are applied once per account.
        self.accounts.sort_unstable_by_key(|read| read.address);
        self.accounts.dedup_by_key(|read| read.address);

        let Ok(caller_index) = self.accounts.binary_search_by_key(&caller, |read| read.address)
        else {
            return Err(self)
        };
        let caller_info = &self.accounts[caller_index].info;
        let (caller_nonce, caller_balance) =
            caller_info.as_ref().map_or((0, U256::ZERO), |info| (info.nonce, info.balance));
        if self.unknown_code ||
            caller_nonce != nonce ||
            nonce == u64::MAX ||
            max_balance_spending.is_none_or(|max| caller_balance < max)
        {
            return Err(self)
        }

        // Rebasing only bounded balances keeps every balance during execution far from overflow,
        // so no value transfer can overflow in one execution but not the other.
        let bounded = self
            .accounts
            .iter()
            .all(|read| read.info.as_ref().is_none_or(|info| info.balance <= MAX_REBASED_BALANCE));
        let paid = caller_balance - max_balance_spending.unwrap_or_default();
        for read in &mut self.accounts {
            // A sender needs enough balance to leave the same smallest balance, which is the
            // balance after paying for gas for the caller.
            let left = balances
                .senders
                .iter()
                .filter(|(address, _)| *address == read.address)
                .map(|(_, left)| *left)
                .chain((read.address == caller).then_some(paid))
                .min();
            read.balance = if !bounded || balances.observed.contains(&read.address) {
                BalanceDependency::Exact
            } else if let Some(left) = left {
                let balance = read.info.as_ref().map_or(U256::ZERO, |info| info.balance);
                BalanceDependency::AtLeast(balance.saturating_sub(left))
            } else {
                BalanceDependency::None
            };
        }
        Ok(self)
    }

    /// Returns whether executing the transaction at `index` against `predicted` would read the
    /// same values, as far as the execution depends on them.
    fn matches(&self, predicted: &PredictedState, index: usize) -> bool {
        self.accounts.iter().all(|read| match predicted.account(read.address, index) {
            Some(account) => read.compare(account.as_ref()).is_some(),
            None => !read.predicted,
        }) && self.storage.iter().all(|read| {
            match predicted.storage(read.address, read.slot, index) {
                Some(value) => value == read.value,
                None => !read.predicted,
            }
        })
    }
}

/// Writes of a key, sorted by the index of the transaction that made them.
type Writes<T> = Vec<(usize, T)>;

/// State writes predicted by speculative results, by transaction index.
///
/// Speculative executions read the latest write predicted before their transaction, so that
/// transactions depending on earlier ones in the block can still be reused.
#[derive(Debug, Default)]
pub(super) struct PredictedState {
    /// Accounts, `None` if destroyed.
    accounts: DashMap<Address, Writes<Option<AccountInfo>>, DefaultHashBuilder>,
    storage: DashMap<(Address, StorageKey), Writes<StorageValue>, DefaultHashBuilder>,
    /// Transactions that cleared an account's storage by creating or destroying it.
    cleared: DashMap<Address, Writes<()>, DefaultHashBuilder>,
    /// Bytecode deployed by predicted writes.
    code: DashMap<B256, Bytecode, DefaultHashBuilder>,
    /// Number of transactions whose actual writes block execution published.
    generation: AtomicUsize,
    /// Whether any account's storage was cleared, so that storage reads can skip `cleared`.
    any_cleared: AtomicBool,
}

impl PredictedState {
    /// Returns the number of transactions whose actual writes block execution published.
    pub(super) fn generation(&self) -> usize {
        self.generation.load(Ordering::Relaxed)
    }

    /// Returns the latest account write predicted before the transaction at `index`.
    fn account(&self, address: Address, index: usize) -> Option<Option<AccountInfo>> {
        let writes = self.accounts.get(&address)?;
        latest(&writes, index).map(|(_, account)| account.clone())
    }

    /// Returns the latest storage value predicted before the transaction at `index`.
    fn storage(&self, address: Address, slot: StorageKey, index: usize) -> Option<StorageValue> {
        let cleared = self
            .any_cleared
            .load(Ordering::Relaxed)
            .then(|| self.cleared.get(&address).and_then(|w| latest(&w, index).map(|(i, _)| *i)))
            .flatten();
        let written = self.storage.get(&(address, slot)).and_then(|w| latest(&w, index).copied());
        match (written, cleared) {
            (Some((written, value)), Some(cleared)) if written >= cleared => Some(value),
            (_, Some(_)) => Some(StorageValue::ZERO),
            (written, None) => written.map(|(_, value)| value),
        }
    }

    /// Returns predicted bytecode by its hash.
    fn code(&self, code_hash: B256) -> Option<Bytecode> {
        self.code.get(&code_hash).map(|code| code.clone())
    }

    /// Publishes the writes of the transaction at `index`.
    fn publish(&self, index: usize, state: &EvmState) {
        self.update(index, state, true);
        for account in state.values() {
            if account.is_touched() &&
                account.is_created() &&
                let Some(code) = &account.info.code
            {
                self.code.insert(account.info.code_hash, code.clone());
            }
        }
    }

    /// Publishes the account writes of the transaction at `index`.
    fn publish_accounts(&self, index: usize, state: &EvmState) {
        for (address, account) in state {
            if account.is_touched() && !account.is_selfdestructed() && !account.info.is_empty() {
                write(
                    &mut self.accounts.entry(*address).or_default(),
                    index,
                    Some(Some(account.info.copy_without_code())),
                );
            }
        }
    }

    /// Retracts the writes of the transaction at `index` that were published from `state`.
    fn retract(&self, index: usize, state: &EvmState) {
        self.update(index, state, false);
    }

    /// Retracts the writes of the transaction at `index` that were published from `previous` but
    /// that `state` does not make.
    fn retract_unwritten(&self, index: usize, previous: &EvmState, state: &EvmState) {
        for (address, account) in previous {
            if !account.is_touched() {
                continue
            }
            let Some(written) = state.get(address).filter(|account| account.is_touched()) else {
                self.retract(index, &core::iter::once((*address, account.clone())).collect());
                continue
            };
            let clears = |account: &Account| {
                account.is_selfdestructed() || account.info.is_empty() || account.is_created()
            };
            if clears(account) && !clears(written) {
                write::<()>(&mut self.cleared.entry(*address).or_default(), index, None);
            }
            // Storage of destroyed accounts is not published.
            let destroyed = written.is_selfdestructed() || written.info.is_empty();
            for (slot, _) in account.changed_storage_slots() {
                if destroyed || !written.storage.get(slot).is_some_and(|value| value.is_changed()) {
                    write::<StorageValue>(
                        &mut self.storage.entry((*address, *slot)).or_default(),
                        index,
                        None,
                    );
                }
            }
        }
    }

    /// Publishes or retracts the writes of the transaction at `index` to every key that `state`
    /// writes.
    fn update(&self, index: usize, state: &EvmState, publish: bool) {
        for (address, account) in state {
            if !account.is_touched() {
                continue
            }
            // Touched empty accounts are removed by EIP-161 state clearing.
            let destroyed = account.is_selfdestructed() || account.info.is_empty();
            if destroyed || account.is_created() {
                self.any_cleared.store(true, Ordering::Relaxed);
                write(&mut self.cleared.entry(*address).or_default(), index, publish.then_some(()));
            }
            // Created bytecode is kept separately, the rest is known to the database.
            let info = publish.then(|| (!destroyed).then(|| account.info.copy_without_code()));
            write(&mut self.accounts.entry(*address).or_default(), index, info);
            if destroyed {
                continue
            }
            for (slot, value) in account.changed_storage_slots() {
                write(
                    &mut self.storage.entry((*address, *slot)).or_default(),
                    index,
                    publish.then_some(value.present_value),
                );
            }
        }
    }
}

/// Sets the write of the transaction at `index`, or removes it if `value` is `None`.
fn write<T>(writes: &mut Writes<T>, index: usize, value: Option<T>) {
    match (writes.binary_search_by_key(&index, |(i, _)| *i), value) {
        (Ok(i), Some(value)) => writes[i].1 = value,
        (Err(i), Some(value)) => writes.insert(i, (index, value)),
        (Ok(i), None) => {
            writes.remove(i);
        }
        (Err(_), None) => {}
    }
}

/// Returns the latest write before the transaction at `index`.
fn latest<T>(writes: &[(usize, T)], index: usize) -> Option<&(usize, T)> {
    writes[..writes.partition_point(|(i, _)| *i < index)].last()
}

/// Database wrapper that records reads for [`PrewarmResults`].
#[derive(Debug)]
pub(super) struct RecordingDb<DB> {
    inner: DB,
    /// Reads of the current transaction, `None` if recording is disabled.
    reads: Option<ReadSet>,
    /// The predicted state, and the index of the current transaction.
    predicted: Option<(Arc<PredictedState>, usize)>,
}

impl<DB> RecordingDb<DB> {
    /// Wraps `inner` without recording.
    pub(super) const fn new(inner: DB) -> Self {
        Self { inner, reads: None, predicted: None }
    }

    /// Starts recording the reads of the transaction at `index`, which reads `predicted` writes
    /// of earlier transactions.
    pub(super) fn start_recording(&mut self, predicted: Arc<PredictedState>, index: usize) {
        self.reads = Some(ReadSet::default());
        self.predicted = Some((predicted, index));
    }

    /// Stops recording and returns the reads of the current transaction.
    pub(super) fn take_reads(&mut self) -> Option<ReadSet> {
        self.predicted = None;
        self.reads.take()
    }
}

impl<DB: Database> Database for RecordingDb<DB> {
    type Error = DB::Error;

    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        let predicted =
            self.predicted.as_ref().and_then(|(state, index)| state.account(address, *index));
        let is_predicted = predicted.is_some();
        let account = match predicted {
            Some(account) => account,
            None => self.inner.basic(address)?,
        };

        if let Some(reads) = &mut self.reads {
            reads.accounts.push(AccountRead {
                address,
                info: account.as_ref().map(AccountInfo::copy_without_code),
                balance: BalanceDependency::None,
                predicted: is_predicted,
            });
        }
        Ok(account)
    }

    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        if let Some(code) = self.predicted.as_ref().and_then(|(state, _)| state.code(code_hash)) {
            return Ok(code)
        }
        let code = self.inner.code_by_hash(code_hash)?;
        // The database returns empty bytecode for unknown hashes.
        if code.is_empty() &&
            code_hash != KECCAK_EMPTY &&
            let Some(reads) = &mut self.reads
        {
            reads.unknown_code = true;
        }
        Ok(code)
    }

    fn storage(
        &mut self,
        address: Address,
        index: StorageKey,
    ) -> Result<StorageValue, Self::Error> {
        let predicted = self
            .predicted
            .as_ref()
            .and_then(|(state, tx_index)| state.storage(address, index, *tx_index));
        let is_predicted = predicted.is_some();
        let value = match predicted {
            Some(value) => value,
            None => self.inner.storage(address, index)?,
        };
        if let Some(reads) = &mut self.reads {
            reads.storage.push(StorageRead {
                address,
                slot: index,
                value,
                predicted: is_predicted,
            });
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
    /// Accounts that sent value, with the balance they had left after each send.
    senders: Vec<(Address, U256)>,
}

impl BalanceTracker {
    /// Clears the tracked accounts for a new transaction.
    pub(super) fn clear(&mut self) {
        self.observed.clear();
        self.senders.clear();
    }

    /// Records that `sender` sends `value`, or observes its balance if it cannot.
    fn send<CTX: ContextTr>(&mut self, context: &mut CTX, sender: Address, value: U256) {
        // The sender of a frame is always loaded, so this does not read the database.
        let left = context
            .journal_mut()
            .load_account(sender)
            .ok()
            .and_then(|account| account.data.info.balance.checked_sub(value));
        match left {
            Some(left) => self.senders.push((sender, left)),
            None => self.observed.push(sender),
        }
    }
}

impl<CTX: ContextTr> Inspector<CTX> for BalanceTracker {
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

    fn call(&mut self, context: &mut CTX, inputs: &mut CallInputs) -> Option<CallOutcome> {
        if let Some(value) = inputs.transfer_value().filter(|value| !value.is_zero()) {
            self.send(context, inputs.transfer_from(), value);
        }
        None
    }

    fn call_end(&mut self, _context: &mut CTX, inputs: &CallInputs, outcome: &mut CallOutcome) {
        if *outcome.instruction_result() == InstructionResult::OutOfFunds {
            self.observed.push(inputs.transfer_from());
        }
    }

    fn create(&mut self, context: &mut CTX, inputs: &mut CreateInputs) -> Option<CreateOutcome> {
        if !inputs.value().is_zero() {
            self.send(context, inputs.caller(), inputs.value());
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
    use alloy_evm::{
        block::{BlockExecutor, TxResult},
        eth::EthBlockExecutionCtx,
        Evm, EvmEnv,
    };
    use alloy_primitives::{bytes, Bytes, Signature, TxKind};
    use reth_ethereum_primitives::{Receipt, Transaction, TransactionSigned};
    use reth_evm::{ConfigureEvm, EvmFor, HaltReasonFor};
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
    /// Sends 5 wei to [`NOOP`].
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
            (FORWARDER, bytes!("5f5f5f5f600573c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c05af100")),
        ] {
            let code = Bytecode::new_raw(code);
            db.insert_account_info(
                address,
                AccountInfo {
                    balance: U256::from(12),
                    code_hash: code.hash_slow(),
                    code: Some(code),
                    ..Default::default()
                },
            );
        }
        db
    }

    /// Speculative execution of `txs`, as by prewarm workers.
    struct Prewarm<'a> {
        txs: &'a [Recovered<TransactionSigned>],
        results: Results,
        evm: EvmFor<EthEvmConfig, RecordingDb<CacheDB<EmptyDB>>, BalanceTracker>,
    }

    impl<'a> Prewarm<'a> {
        /// Executes the transactions at the indices in `order`, then refreshes stale results
        /// among them in block order.
        fn new(txs: &'a [Recovered<TransactionSigned>], order: &[usize]) -> Self {
            let evm = EthEvmConfig::mainnet().evm_with_env_and_inspector(
                RecordingDb::new(parent_state()),
                evm_env(true),
                BalanceTracker::default(),
            );
            let mut prewarm = Self { txs, results: Results::new(txs.len()), evm };
            for &index in order {
                prewarm.run(index);
            }
            let mut refresh = order.to_vec();
            refresh.sort_unstable();
            for index in refresh {
                if prewarm.results.needs_refresh(index) {
                    prewarm.run(index);
                }
            }
            prewarm
        }

        fn run(&mut self, index: usize) {
            let tx = &self.txs[index];
            let tx_env = EthEvmConfig::mainnet().tx_env(tx);
            let (nonce, max_balance_spending) = (tx_env.nonce, tx_env.max_balance_spending().ok());
            self.evm.db_mut().start_recording(Arc::clone(self.results.predicted()), index);
            self.evm.inspector_mut().clear();
            let result = self.evm.transact(tx_env).unwrap();
            let reads = self.evm.db_mut().take_reads().unwrap();
            match reads.finish(self.evm.inspector(), tx.signer(), nonce, max_balance_spending) {
                Ok(reads) => self.results.insert(index, result, reads),
                Err(reads) => self.results.reject(index, reads),
            }
        }

        /// Refreshes stale or missing results of transactions after `index`.
        fn refresh_after(&mut self, index: usize) {
            for index in index + 1..self.txs.len() {
                if self.results.needs_refresh(index) {
                    self.run(index);
                }
            }
        }
    }

    /// Executes `txs` sequentially, committing valid prewarm results instead of executing.
    ///
    /// If `publish` is set, prewarm results are refreshed against the writes of executed
    /// transactions.
    fn execute(
        txs: &[Recovered<TransactionSigned>],
        mut prewarm: Option<&mut Prewarm<'_>>,
        publish: bool,
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
            let Some(prewarm) = prewarm.as_deref_mut() else {
                executor.execute_transaction(tx).unwrap();
                outcomes.push(Err(ReuseMiss::Missing));
                continue
            };
            let outcome = match prewarm.results.take_valid(index, executor.evm_mut().db_mut()) {
                Ok(result) => {
                    executor
                        .commit_transaction(config.prewarm_tx_result(tx.inner(), result).unwrap());
                    Ok(())
                }
                Err(miss) if publish => {
                    executor
                        .execute_transaction_with_result_closure(tx, |result| {
                            prewarm.results.publish_executed(index, &result.result().state)
                        })
                        .unwrap();
                    prewarm.refresh_after(index);
                    Err(miss)
                }
                Err(miss) => {
                    executor.execute_transaction(tx).unwrap();
                    Err(miss)
                }
            };
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
        reuse_in(txs, &(0..txs.len()).collect::<Vec<_>>(), true)
    }

    /// Like [`reuse`], prewarming only the transactions at the indices in `order`, and refreshing
    /// results against the writes of executed transactions if `publish` is set.
    fn reuse_in(
        txs: &[Recovered<TransactionSigned>],
        order: &[usize],
        publish: bool,
    ) -> Vec<Result<(), ReuseMiss>> {
        let (receipts, bundle, outcomes) =
            execute(txs, Some(&mut Prewarm::new(txs, order)), publish);
        let (expected_receipts, expected_bundle, _) = execute(txs, None, false);
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
        // The last transaction's caller and recipient were credited by earlier ones.
        let txs = [tx(1, 0, NOOP, 1), tx(1, 1, sender(2), 1), tx(2, 0, NOOP, 1)];
        assert_eq!(reuse(&txs), [Ok(()), Ok(()), Ok(())]);
    }

    #[test]
    fn reuses_predicted_writes() {
        let txs = [tx(1, 0, COUNTER, 1), tx(2, 0, COUNTER, 1), tx(1, 1, COUNTER, 1)];
        assert_eq!(reuse(&txs), [Ok(()), Ok(()), Ok(())]);
        // Results executed before the transactions they depend on are executed again.
        assert_eq!(reuse_in(&txs, &[2, 1, 0], true), [Ok(()), Ok(()), Ok(())]);
    }

    #[test]
    fn refreshes_against_executed_writes() {
        let txs = [tx(1, 0, COUNTER, 1), tx(2, 0, COUNTER, 1), tx(1, 1, sender(3), 1)];
        assert_eq!(reuse_in(&txs, &[1, 2], true), [Err(ReuseMiss::Missing), Ok(()), Ok(())]);
    }

    #[test]
    fn rejects_stale_storage() {
        let txs = [tx(1, 0, COUNTER, 1), tx(2, 0, COUNTER, 1)];
        assert_eq!(reuse_in(&txs, &[1], false), [Err(ReuseMiss::Missing), Err(ReuseMiss::Stale)]);
    }

    #[test]
    fn rejects_stale_callers() {
        let txs = [tx(1, 0, sender(2), 1), tx(1, 1, sender(3), 1)];
        assert_eq!(reuse_in(&txs, &[1], false), [Err(ReuseMiss::Missing), Err(ReuseMiss::Stale)]);
    }

    #[test]
    fn rejects_observed_balances() {
        let txs = [
            tx(1, 0, SELF_BALANCE_READER, 1),
            tx(2, 0, SELF_BALANCE_READER, 1),
            tx(3, 0, BENEFICIARY_BALANCE_READER, 1),
        ];
        assert_eq!(
            reuse_in(&txs, &[1, 2], false),
            [Err(ReuseMiss::Missing), Err(ReuseMiss::Stale), Err(ReuseMiss::Stale)]
        );
    }

    #[test]
    fn rejects_lower_sender_balances() {
        let txs = [tx(1, 0, FORWARDER, 0), tx(2, 0, FORWARDER, 0), tx(3, 0, FORWARDER, 0)];
        // The forwarder can send twice from its speculative balance, but not three times.
        assert_eq!(
            reuse_in(&txs, &[1, 2], false),
            [Err(ReuseMiss::Missing), Ok(()), Err(ReuseMiss::Stale)]
        );
    }
}

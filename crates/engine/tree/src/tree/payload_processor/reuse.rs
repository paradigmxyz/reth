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
//! transfers. Successful value transfers and gas payments only require enough starting balance
//! to cover the largest cumulative debit recorded during speculation.
//!
//! Prewarming also seeds results of pool transactions that the txpool prewarm worker executed on
//! top of the parent before the block arrived, see [`PoolSeeds`]. Their block environment is
//! predicted, so they must not depend on fields that turn out to differ, and their fee credit to
//! the beneficiary is applied to the actual beneficiary when the result is reused.

use alloy_primitives::{
    address,
    map::{B256Map, DefaultHashBuilder},
    Address, B256, U256,
};
use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use revm::{
    bytecode::opcode,
    context::{
        result::{InvalidTransaction, ResultAndState},
        Block, ContextTr, JournalTr,
    },
    inspector::JournalExt,
    interpreter::{
        instructions::utility::IntoAddress, interpreter_types::Jumps, CallInputs, CallOutcome,
        CreateInputs, CreateOutcome, InstructionResult, Interpreter,
    },
    primitives::{StorageKey, StorageValue, KECCAK_EMPTY},
    state::{Account, AccountInfo, Bytecode, EvmState},
    Database, Inspector,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

/// Balances above this are compared exactly, so that applying balance changes cannot overflow.
const MAX_REBASED_BALANCE: U256 = U256::from_limbs([u64::MAX, u64::MAX, 0, 0]);

/// Beneficiary of speculative pool transaction executions.
///
/// Results of transactions that access it are discarded, so its value only affects how many are.
pub(crate) const POOL_BENEFICIARY: Address = address!("0x00000000000000000000000000000000000beef0");

/// Maximum number of reads and writes held by [`PoolResults`].
const MAX_POOL_RESULT_ENTRIES: usize = 1 << 18;

/// Speculative prewarm results of a block's transactions, indexed by their position in the block.
#[derive(Debug)]
pub(crate) struct PrewarmResults<H> {
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
    pub(crate) fn new(transaction_count: usize) -> Self {
        Self {
            slots: (0..transaction_count).map(|_| Mutex::new(ResultSlot::Empty)).collect(),
            predicted: Arc::default(),
        }
    }

    /// Returns the writes predicted by the stored results.
    pub(super) const fn predicted(&self) -> &Arc<PredictedState> {
        &self.predicted
    }

    /// Stores the prewarm result of the transaction at `index` and publishes its writes if
    /// [`ReadSet::finish`] accepts its reads, otherwise marks it as unusable.
    pub(super) fn store(
        &self,
        index: usize,
        result: ResultAndState<H>,
        reads: ReadSet,
        balances: &BalanceTracker,
        tx: SpeculativeTx,
    ) {
        match reads.finish(balances, tx) {
            Ok(reads) => self.set(index, |predicted| {
                predicted.publish(index, &result.state);
                ResultSlot::Ready(Box::new(PrewarmedTx { result, reads, credit: None }))
            }),
            Err(reads) => self.reject(index, reads),
        }
    }

    /// Stores the result of the pool transaction with `hash` as the prewarm result of the
    /// transaction at `index` and publishes its writes, unless prewarming already executed it or
    /// the result depends on the block environment fields that `seeds` differ in.
    ///
    /// Returns the pool transaction if it was stored.
    pub(super) fn seed(
        &self,
        index: usize,
        hash: &B256,
        seeds: &PoolSeeds<H>,
    ) -> Option<Arc<PoolTx<H>>>
    where
        H: Clone,
    {
        let tx = seeds.results.get(hash)?;
        // The speculative execution treated the actual beneficiary as a cold account.
        if tx.reads.block_env.intersects(seeds.differs) ||
            tx.reads.position(seeds.beneficiary).is_some()
        {
            return None
        }
        let mut slot = self.slots.get(index)?.lock();
        if !matches!(*slot, ResultSlot::Empty) {
            return None
        }
        self.predicted.publish(index, &tx.result.state);
        *slot = ResultSlot::Ready(Box::new(PrewarmedTx {
            result: tx.result.clone(),
            reads: tx.reads.clone(),
            credit: Some(BeneficiaryCredit { beneficiary: seeds.beneficiary, amount: tx.credit }),
        }));
        Some(tx)
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
    pub(crate) fn publish_executed(&self, index: usize, state: &EvmState) {
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

    /// Takes the prewarm result of the transaction at `index` if it is ready and its reads match
    /// `db`.
    ///
    /// The returned result has its balance changes applied to the current balances. Stale results,
    /// and results whose reads fail, stay in their slot unchanged and are freed with `self`.
    pub(crate) fn take_valid<DB: Database>(
        &self,
        index: usize,
        db: &mut DB,
    ) -> Result<Result<ResultAndState<H>, ReuseMiss>, DB::Error> {
        let Some(slot) = self.slots.get(index) else { return Ok(Err(ReuseMiss::Missing)) };
        let mut slot = slot.lock();
        let mut tx = match core::mem::replace(&mut *slot, ResultSlot::Executed) {
            ResultSlot::Ready(tx) => tx,
            ResultSlot::Unusable(reads) => {
                *slot = ResultSlot::Unusable(reads);
                return Ok(Err(ReuseMiss::Stale))
            }
            previous @ (ResultSlot::Empty | ResultSlot::Executed) => {
                *slot = previous;
                return Ok(Err(ReuseMiss::Missing))
            }
        };
        let rebased = match tx.validate(db) {
            Ok(Some(rebased)) => rebased,
            result => {
                *slot = ResultSlot::Ready(tx);
                return result.map(|_| Err(ReuseMiss::Stale))
            }
        };
        // The result stays published as the prediction of this transaction's writes.
        if rebased {
            // Later transactions read the rebased balances.
            self.predicted.publish_accounts(index, &tx.result.state);
            self.predicted.generation.fetch_add(1, Ordering::Relaxed);
        }
        Ok(Ok(tx.result))
    }
}

/// Reason a prewarm result could not be reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReuseMiss {
    /// Prewarming had not finished the transaction.
    Missing,
    /// The speculative result was unusable or failed a reuse eligibility check.
    Stale,
}

/// A speculative transaction result and the database reads it depends on.
#[derive(Debug)]
struct PrewarmedTx<H> {
    result: ResultAndState<H>,
    reads: ReadSet,
    /// The fee credit to apply to the block beneficiary, for results of pool transactions.
    credit: Option<BeneficiaryCredit>,
}

/// A fee credit to the block beneficiary that a speculative result does not contain.
#[derive(Debug)]
struct BeneficiaryCredit {
    beneficiary: Address,
    amount: U256,
}

impl<H> PrewarmedTx<H> {
    /// Returns whether all reads match `db`, applying the result's balance changes to the current
    /// balances of accounts whose balance differs and its credit to the beneficiary, and whether
    /// any account changed.
    ///
    /// The result is left unchanged unless this returns `Some`, so that it can be validated again.
    fn validate<DB: Database>(&mut self, db: &mut DB) -> Result<Option<bool>, DB::Error> {
        for read in &self.reads.storage {
            if db.storage(read.address, read.slot)? != read.value {
                return Ok(None)
            }
        }
        let mut rebased = Vec::new();
        for read in &self.reads.accounts {
            let current = db.basic(read.address)?;
            match read.compare(current.as_ref()) {
                AccountMatch::Same => {}
                AccountMatch::Rebase { speculative, current } => {
                    if let Some(account) = self.result.state.get(&read.address) {
                        // All balances are bounded, and the execution never leaves less than the
                        // difference, so this neither overflows nor underflows.
                        let Some(balance) = account
                            .info
                            .balance
                            .checked_add(current)
                            .and_then(|balance| balance.checked_sub(speculative))
                        else {
                            return Ok(None)
                        };
                        rebased.push(Rebase { address: read.address, balance, original: current });
                    }
                }
                AccountMatch::Differs => return Ok(None),
            }
        }
        let credited = match &self.credit {
            Some(credit) => {
                // The beneficiary is not in the result, since the transaction did not read it.
                // Like execution, load it and touch it even if the credit is zero. A missing
                // beneficiary would be loaded as not existing, which is not worth supporting.
                let Some(info) = db.basic(credit.beneficiary)? else { return Ok(None) };
                let mut account = Account::from(info);
                // Execution leaves the balance unchanged on overflow.
                let Some(balance) = account.info.balance.checked_add(credit.amount) else {
                    return Ok(None)
                };
                account.info.balance = balance;
                account.mark_touch();
                Some((credit.beneficiary, account))
            }
            None => None,
        };
        for rebase in &rebased {
            if let Some(account) = self.result.state.get_mut(&rebase.address) {
                account.info.balance = rebase.balance;
                // The state root task only streams accounts whose info differs from the original.
                account.original_info_mut().balance = rebase.original;
            }
        }
        let changed = !rebased.is_empty() || credited.is_some();
        self.result.state.extend(credited);
        Ok(Some(changed))
    }
}

/// A balance of a speculative result applied to the current balance.
#[derive(Debug)]
struct Rebase {
    address: Address,
    /// The balance after the transaction.
    balance: U256,
    /// The balance before the transaction.
    original: U256,
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
#[derive(Debug, Clone)]
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
    fn compare(&self, current: Option<&AccountInfo>) -> AccountMatch {
        let (speculative, current) = match (&self.info, current) {
            (Some(speculative), Some(current)) => (speculative, current),
            (None, None) => return AccountMatch::Same,
            _ => return AccountMatch::Differs,
        };
        if speculative.balance == current.balance {
            return if speculative == current { AccountMatch::Same } else { AccountMatch::Differs }
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
            return AccountMatch::Differs
        }
        AccountMatch::Rebase { speculative: speculative.balance, current: current.balance }
    }
}

/// How a read account compares with its current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccountMatch {
    /// The account is unchanged.
    Same,
    /// Only the balance changed, so the result's balance change must be applied to the current
    /// balance.
    Rebase { speculative: U256, current: U256 },
    /// The account differs in a way the execution may depend on.
    Differs,
}

/// A storage slot a speculative execution read.
#[derive(Debug, Clone)]
struct StorageRead {
    address: Address,
    slot: StorageKey,
    value: StorageValue,
    /// Whether the slot was read from the predicted state.
    predicted: bool,
}

/// Database reads of a speculative transaction execution.
#[derive(Debug, Default, Clone)]
pub(crate) struct ReadSet {
    accounts: Vec<AccountRead>,
    storage: Vec<StorageRead>,
    /// Whether execution loaded bytecode that is unknown to the database.
    unknown_code: bool,
    /// Block environment fields the execution observed.
    block_env: BlockEnvFields,
}

impl ReadSet {
    /// Classifies how the execution depends on the balance of each account it read.
    ///
    /// Speculation disables nonce and balance checks, so this fails unless `caller` had the
    /// transaction's `nonce` and could pay `max_balance_spending`, i.e. unless execution with
    /// these checks enabled would have accepted the transaction.
    fn finish(mut self, balances: &BalanceTracker, tx: SpeculativeTx) -> Result<Self, Self> {
        let SpeculativeTx { caller, nonce, max_balance_spending } = tx;
        // Balance changes are applied once per account.
        self.accounts.sort_unstable_by_key(|read| read.address);
        self.accounts.dedup_by_key(|read| read.address);
        self.block_env = balances.block_env;

        let Ok(caller_index) = self.accounts.binary_search_by_key(&caller, |read| read.address)
        else {
            return Err(self)
        };
        let caller_info = &self.accounts[caller_index].info;
        let (caller_nonce, caller_balance) =
            caller_info.as_ref().map_or((0, U256::ZERO), |info| (info.nonce, info.balance));
        let Ok(max_balance_spending) = max_balance_spending else { return Err(self) };
        // Speculation still rejects transactions with the largest nonce, in revm's `validate_env`.
        if self.unknown_code || caller_nonce != nonce || caller_balance < max_balance_spending {
            return Err(self)
        }

        // Rebasing only bounded balances keeps every balance during execution far from overflow,
        // so no value transfer can overflow in one execution but not the other.
        let bounded = self
            .accounts
            .iter()
            .all(|read| read.info.as_ref().is_none_or(|info| info.balance <= MAX_REBASED_BALANCE));
        let paid = caller_balance - max_balance_spending;
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

    /// Returns the position of the read of `address` in the sorted account reads.
    fn position(&self, address: Address) -> Option<usize> {
        self.accounts.binary_search_by_key(&address, |read| read.address).ok()
    }

    /// Returns whether executing the transaction at `index` against `predicted` would read the
    /// same values, as far as the execution depends on them.
    fn matches(&self, predicted: &PredictedState, index: usize) -> bool {
        self.accounts.iter().all(|read| match predicted.account(read.address, index) {
            Some(account) => read.compare(account.as_ref()) != AccountMatch::Differs,
            None => !read.predicted,
        }) && self.storage.iter().all(|read| {
            match predicted.storage(read.address, read.slot, index) {
                Some(value) => value == read.value,
                None => !read.predicted,
            }
        })
    }
}

/// The fields of a speculatively executed transaction that its validity checks depend on.
#[derive(Debug)]
pub(crate) struct SpeculativeTx {
    /// The transaction's sender.
    pub(crate) caller: Address,
    /// The transaction's nonce.
    pub(crate) nonce: u64,
    /// The most the caller may spend, or an error if it overflows.
    pub(crate) max_balance_spending: Result<U256, InvalidTransaction>,
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
    /// Number of actual execution or rebased account updates published by block execution.
    generation: AtomicUsize,
}

impl PredictedState {
    /// Returns the number of actual execution or rebased account updates published by block
    /// execution.
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
        let cleared = self.cleared.get(&address).and_then(|w| latest(&w, index).map(|(i, _)| *i));
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
                self.update(index, [(address, account)], false);
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
    fn update<'a>(
        &self,
        index: usize,
        state: impl IntoIterator<Item = (&'a Address, &'a Account)>,
        publish: bool,
    ) {
        for (address, account) in state {
            if !account.is_touched() {
                continue
            }
            // Touched empty accounts are removed by EIP-161 state clearing.
            let destroyed = account.is_selfdestructed() || account.info.is_empty();
            if destroyed || account.is_created() {
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

/// Block environment fields that speculative executions may observe and that may differ from the
/// actual block environment.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlockEnvFields {
    /// The beneficiary, observed by `COINBASE` or by accessing its account.
    pub(crate) beneficiary: bool,
    /// The timestamp, observed by `TIMESTAMP`.
    pub(crate) timestamp: bool,
    /// The prevrandao, observed by `PREVRANDAO`.
    pub(crate) prevrandao: bool,
    /// The gas limit, observed by `GASLIMIT`.
    pub(crate) gas_limit: bool,
}

impl BlockEnvFields {
    /// Returns whether any field is set in both `self` and `other`.
    const fn intersects(self, other: Self) -> bool {
        (self.beneficiary && other.beneficiary) ||
            (self.timestamp && other.timestamp) ||
            (self.prevrandao && other.prevrandao) ||
            (self.gas_limit && other.gas_limit)
    }
}

/// Results of pool transactions executed on top of a block's parent in a predicted block
/// environment with [`POOL_BENEFICIARY`] as the beneficiary, by transaction hash.
#[derive(Debug)]
pub(crate) struct PoolResults<H> {
    txs: RwLock<B256Map<Arc<PoolTx<H>>>>,
    /// Number of reads and writes of the stored results.
    entries: AtomicUsize,
}

impl<H> Default for PoolResults<H> {
    fn default() -> Self {
        Self { txs: RwLock::default(), entries: AtomicUsize::default() }
    }
}

impl<H> PoolResults<H> {
    /// Stores the result of the pool transaction with `hash` if [`ReadSet::finish`] accepts its
    /// reads, unless it accessed the beneficiary or the stored results would hold more than
    /// [`MAX_POOL_RESULT_ENTRIES`] reads and writes.
    ///
    /// Returns whether the result was stored.
    pub(crate) fn store(
        &self,
        hash: B256,
        mut result: ResultAndState<H>,
        reads: ReadSet,
        balances: &BalanceTracker,
        tx: SpeculativeTx,
    ) -> bool {
        if balances.block_env.beneficiary {
            return false
        }
        let Ok(mut reads) = reads.finish(balances, tx) else { return false };

        // The beneficiary was only loaded to credit fees, which are credited to the actual
        // beneficiary instead.
        let Some(beneficiary) = result.state.remove(&POOL_BENEFICIARY) else { return false };
        let Some(index) = reads.position(POOL_BENEFICIARY) else { return false };
        let speculative = reads.accounts.remove(index).info.map_or(U256::ZERO, |info| info.balance);
        let Some(credit) = beneficiary.info.balance.checked_sub(speculative) else { return false };

        let entries = reads.accounts.len() +
            reads.storage.len() +
            result.state.values().map(|account| 1 + account.storage.len()).sum::<usize>();
        let mut txs = self.txs.write();
        let total = self.entries.load(Ordering::Relaxed) + entries;
        if total > MAX_POOL_RESULT_ENTRIES {
            return false
        }
        txs.insert(hash, Arc::new(PoolTx { result, reads, credit }));
        self.entries.store(total, Ordering::Relaxed);
        true
    }

    /// Returns the result of the pool transaction with `hash`.
    pub(crate) fn get(&self, hash: &B256) -> Option<Arc<PoolTx<H>>> {
        self.txs.read().get(hash).cloned()
    }
}

/// A speculative result of a pool transaction without its fee credit to the beneficiary.
#[derive(Debug)]
pub(crate) struct PoolTx<H> {
    result: ResultAndState<H>,
    reads: ReadSet,
    /// The fee credit to the beneficiary.
    credit: U256,
}

impl<H> PoolTx<H> {
    /// Returns the state changes of the transaction.
    pub(super) const fn state(&self) -> &EvmState {
        &self.result.state
    }

    /// Loads every read of the transaction from `db`.
    pub(super) fn load_reads<DB: Database>(&self, db: &mut DB) -> Result<(), DB::Error> {
        for read in &self.reads.accounts {
            db.basic(read.address)?;
        }
        for read in &self.reads.storage {
            db.storage(read.address, read.slot)?;
        }
        Ok(())
    }
}

/// Results of pool transactions that may seed the [`PrewarmResults`] of a block on top of the
/// parent they were executed on.
#[derive(Debug, Clone)]
pub struct PoolSeeds<H> {
    results: Arc<PoolResults<H>>,
    /// The beneficiary of the block.
    beneficiary: Address,
    /// Block environment fields that differ from the predicted environment.
    differs: BlockEnvFields,
}

impl<H> PoolSeeds<H> {
    /// Creates seeds for a block with `beneficiary` whose environment differs from the predicted
    /// environment of `results` in `differs`.
    pub(crate) const fn new(
        results: Arc<PoolResults<H>>,
        beneficiary: Address,
        differs: BlockEnvFields,
    ) -> Self {
        Self { results, beneficiary, differs }
    }
}

/// Database wrapper that records reads for [`PrewarmResults`].
#[derive(Debug)]
pub(crate) struct RecordingDb<DB> {
    inner: DB,
    /// Reads of the current transaction, `None` if recording is disabled.
    reads: Option<ReadSet>,
    /// The predicted state, and the index of the current transaction.
    predicted: Option<(Arc<PredictedState>, usize)>,
}

impl<DB> RecordingDb<DB> {
    /// Wraps `inner` without recording.
    pub(crate) const fn new(inner: DB) -> Self {
        Self { inner, reads: None, predicted: None }
    }

    /// Starts recording the reads of the transaction at `index`, which reads `predicted` writes
    /// of earlier transactions.
    pub(super) fn start_recording(&mut self, predicted: Arc<PredictedState>, index: usize) {
        self.reads = Some(ReadSet::default());
        self.predicted = Some((predicted, index));
    }

    /// Starts recording the reads of a transaction that only reads the wrapped database.
    pub(crate) fn start_recording_inner(&mut self) {
        self.reads = Some(ReadSet::default());
        self.predicted = None;
    }

    /// Stops recording and returns the reads of the current transaction.
    pub(crate) fn take_reads(&mut self) -> Option<ReadSet> {
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

/// Inspector that tracks which balances and block environment fields a transaction observes and
/// which accounts send value.
#[derive(Debug, Default)]
pub(crate) struct BalanceTracker {
    /// Accounts whose balance was observed.
    observed: Vec<Address>,
    /// Accounts that sent value, with the balance they had left after each send.
    senders: Vec<(Address, U256)>,
    /// Block environment fields that were observed.
    block_env: BlockEnvFields,
}

impl BalanceTracker {
    /// Clears the tracked accounts for a new transaction.
    pub(crate) fn clear(&mut self) {
        self.observed.clear();
        self.senders.clear();
        self.block_env = BlockEnvFields::default();
    }

    /// Records whether the transaction accessed the beneficiary when its outermost frame ends.
    ///
    /// Otherwise, the beneficiary is only loaded afterwards to credit fees. Loaded accounts stay
    /// loaded when frames revert.
    fn frame_end<CTX: ContextTr<Journal: JournalExt>>(&mut self, context: &CTX) {
        let journal = context.journal_ref();
        if journal.depth() == 0 && journal.evm_state().contains_key(&context.block().beneficiary())
        {
            self.block_env.beneficiary = true;
        }
    }

    /// Records that `sender` sends `value`, or observes its balance if it cannot.
    fn send<CTX: ContextTr<Journal: JournalTr<State = EvmState>>>(
        &mut self,
        context: &CTX,
        sender: Address,
        value: U256,
    ) {
        // The sender of a frame is always loaded.
        let left = context
            .journal_ref()
            .evm_state()
            .get(&sender)
            .and_then(|account| account.info.balance.checked_sub(value));
        match left {
            Some(left) => self.senders.push((sender, left)),
            None => self.observed.push(sender),
        }
    }
}

impl<CTX: ContextTr<Journal: JournalExt> + ContextTr<Journal: JournalTr<State = EvmState>>>
    Inspector<CTX> for BalanceTracker
{
    fn step(&mut self, interp: &mut Interpreter, _context: &mut CTX) {
        let address = match interp.bytecode.opcode() {
            opcode::BALANCE => {
                let Ok(word) = interp.stack.peek(0) else { return };
                word.into_address()
            }
            opcode::SELFBALANCE | opcode::SELFDESTRUCT => interp.input.target_address,
            opcode::COINBASE => {
                self.block_env.beneficiary = true;
                return
            }
            opcode::TIMESTAMP => {
                self.block_env.timestamp = true;
                return
            }
            opcode::DIFFICULTY => {
                self.block_env.prevrandao = true;
                return
            }
            opcode::GASLIMIT => {
                self.block_env.gas_limit = true;
                return
            }
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

    fn call_end(&mut self, context: &mut CTX, inputs: &CallInputs, outcome: &mut CallOutcome) {
        if *outcome.instruction_result() == InstructionResult::OutOfFunds {
            self.observed.push(inputs.transfer_from());
        }
        self.frame_end(context);
    }

    fn create(&mut self, context: &mut CTX, inputs: &mut CreateInputs) -> Option<CreateOutcome> {
        if !inputs.value().is_zero() {
            self.send(context, inputs.caller(), inputs.value());
        }
        None
    }

    fn create_end(
        &mut self,
        context: &mut CTX,
        inputs: &CreateInputs,
        outcome: &mut CreateOutcome,
    ) {
        if *outcome.instruction_result() == InstructionResult::OutOfFunds {
            self.observed.push(inputs.caller());
        }
        self.frame_end(context);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{transaction::Recovered, TxEip1559, TxLegacy};
    use alloy_evm::{
        block::{BlockExecutor, TxResult},
        eth::EthBlockExecutionCtx,
        Evm, EvmEnv,
    };
    use alloy_primitives::{bytes, Bytes, Signature, TxKind};
    use reth_errors::ProviderError;
    use reth_ethereum_primitives::{Receipt, Transaction, TransactionSigned};
    use reth_evm::{ConfigureEvm, EvmFor, HaltReasonFor};
    use reth_evm_ethereum::EthEvmConfig;
    use revm::{
        context::{BlockEnv, CfgEnv, Transaction as _},
        database::{states::bundle_state::BundleRetention, BundleState, CacheDB, EmptyDB, State},
        primitives::hardfork::SpecId,
        state::EvmStorageSlot,
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
    /// Has a balance above [`MAX_REBASED_BALANCE`].
    const RICH: Address = Address::repeat_byte(0xc5);
    /// Exists with a zero balance.
    const ZERO_BALANCE: Address = Address::repeat_byte(0xc6);
    /// Has a code hash whose bytecode the database does not know.
    const UNKNOWN_CODE: Address = Address::repeat_byte(0xc7);
    /// Stores the timestamp in slot 0.
    const TIMESTAMP_READER: Address = Address::repeat_byte(0xc8);
    /// Stores the gas limit in slot 0.
    const GAS_LIMIT_READER: Address = Address::repeat_byte(0xc9);
    /// Stores the beneficiary in slot 0.
    const BENEFICIARY_READER: Address = Address::repeat_byte(0xca);
    /// Calls [`POOL_BENEFICIARY`].
    const POOL_BENEFICIARY_CALLER: Address = Address::repeat_byte(0xcb);
    /// Index of a sender with the largest nonce.
    const EXHAUSTED_SENDER: u8 = 5;
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
        tx_with_gas_limit(from, nonce, to, value, 100_000)
    }

    fn tx_with_gas_limit(
        from: u8,
        nonce: u64,
        to: Address,
        value: u64,
        gas_limit: u64,
    ) -> Recovered<TransactionSigned> {
        let tx = Transaction::Legacy(TxLegacy {
            chain_id: Some(1),
            nonce,
            gas_price: GAS_PRICE,
            gas_limit,
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
        // The exhausted sender is funded separately.
        for i in [1, 2, 3, 4, 6] {
            db.insert_account_info(
                sender(i),
                AccountInfo { balance: U256::from(10u64.pow(18)), ..Default::default() },
            );
        }
        db.insert_account_info(
            sender(EXHAUSTED_SENDER),
            AccountInfo {
                balance: U256::from(10u64.pow(18)),
                nonce: u64::MAX,
                ..Default::default()
            },
        );
        db.insert_account_info(
            BENEFICIARY,
            AccountInfo { balance: U256::from(1), ..Default::default() },
        );
        db.insert_account_info(RICH, AccountInfo { balance: U256::MAX >> 1, ..Default::default() });
        db.insert_account_info(ZERO_BALANCE, AccountInfo { nonce: 1, ..Default::default() });
        db.insert_account_info(
            UNKNOWN_CODE,
            AccountInfo { code_hash: B256::repeat_byte(0xab), code: None, ..Default::default() },
        );
        for (address, code) in [
            (NOOP, bytes!("00")),
            (COUNTER, bytes!("5f546001015f5500")),
            (BENEFICIARY_BALANCE_READER, bytes!("41315f5500")),
            (SELF_BALANCE_READER, bytes!("475f5500")),
            (FORWARDER, bytes!("5f5f5f5f600573c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c05af100")),
            (TIMESTAMP_READER, bytes!("425f5500")),
            (GAS_LIMIT_READER, bytes!("455f5500")),
            (BENEFICIARY_READER, bytes!("415f5500")),
            (
                POOL_BENEFICIARY_CALLER,
                bytes!("5f5f5f5f5f7300000000000000000000000000000000000beef05af100"),
            ),
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
            let speculative = SpeculativeTx {
                caller: tx.signer(),
                nonce: tx_env.nonce,
                max_balance_spending: tx_env.max_balance_spending(),
            };
            self.evm.db_mut().start_recording(Arc::clone(self.results.predicted()), index);
            self.evm.inspector_mut().clear();
            let result = self.evm.transact(tx_env);
            let reads = self.evm.db_mut().take_reads().unwrap();
            match result {
                Ok(result) => {
                    self.results.store(index, result, reads, self.evm.inspector(), speculative)
                }
                Err(_) => self.results.reject(index, reads),
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
        beneficiary: Address,
    ) -> Execution {
        let config = EthEvmConfig::mainnet();
        let mut state = State::builder().with_database(parent_state()).with_bundle_update().build();
        let mut evm_env = evm_env(false);
        evm_env.block_env.beneficiary = beneficiary;
        let evm = config.evm_with_env(&mut state, evm_env);
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
        let mut changes = Vec::new();
        for (index, tx) in txs.iter().enumerate() {
            let Some(prewarm) = prewarm.as_deref_mut() else {
                executor
                    .execute_transaction_with_result_closure(tx, |result| {
                        changes.push(changed_accounts(&result.result().state))
                    })
                    .unwrap();
                outcomes.push(Err(ReuseMiss::Missing));
                continue
            };
            let outcome =
                match prewarm.results.take_valid(index, executor.evm_mut().db_mut()).unwrap() {
                    Ok(result) => {
                        changes.push(changed_accounts(&result.state));
                        executor.commit_transaction(
                            config.prewarm_tx_result(tx.inner(), result).unwrap(),
                        );
                        Ok(())
                    }
                    Err(miss) => {
                        executor
                            .execute_transaction_with_result_closure(tx, |result| {
                                changes.push(changed_accounts(&result.result().state));
                                if publish {
                                    prewarm.results.publish_executed(index, &result.result().state)
                                }
                            })
                            .unwrap();
                        if publish {
                            prewarm.refresh_after(index);
                        }
                        Err(miss)
                    }
                };
            outcomes.push(outcome);
        }
        let receipts = executor.receipts().to_vec();
        drop(executor);
        state.merge_transitions(BundleRetention::Reverts);
        Execution { receipts, bundle: state.take_bundle(), changes, outcomes }
    }

    /// Output of [`execute`].
    struct Execution {
        receipts: Vec<Receipt>,
        bundle: BundleState,
        /// Accounts each transaction changed, as the state root task sees them.
        changes: Vec<Vec<(Address, AccountInfo)>>,
        outcomes: Vec<Result<(), ReuseMiss>>,
    }

    /// Returns the accounts in `state` whose info differs from their original info, which the
    /// state root task streams as updates.
    fn changed_accounts(state: &EvmState) -> Vec<(Address, AccountInfo)> {
        let mut changed = state
            .iter()
            .filter(|(_, account)| account.is_touched() && account.info != account.original_info())
            .map(|(address, account)| (*address, account.info.copy_without_code()))
            .collect::<Vec<_>>();
        changed.sort_unstable_by_key(|(address, _)| *address);
        changed
    }

    /// Database that fails every read.
    struct FailingDb;

    impl Database for FailingDb {
        type Error = ProviderError;

        fn basic(&mut self, _address: Address) -> Result<Option<AccountInfo>, Self::Error> {
            Err(ProviderError::BestBlockNotFound)
        }

        fn code_by_hash(&mut self, _code_hash: B256) -> Result<Bytecode, Self::Error> {
            Err(ProviderError::BestBlockNotFound)
        }

        fn storage(
            &mut self,
            _address: Address,
            _index: StorageKey,
        ) -> Result<StorageValue, Self::Error> {
            Err(ProviderError::BestBlockNotFound)
        }

        fn block_hash(&mut self, _number: u64) -> Result<B256, Self::Error> {
            Err(ProviderError::BestBlockNotFound)
        }
    }

    /// Returns whether the prewarm result of the first transaction can be reused, for
    /// transactions that block execution would reject.
    fn reusable(txs: &[Recovered<TransactionSigned>]) -> Result<(), ReuseMiss> {
        let prewarm = Prewarm::new(txs, &[0]);
        let mut state = State::builder().with_database(parent_state()).build();
        prewarm.results.take_valid(0, &mut state).unwrap().map(drop)
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
        let reused = execute(txs, Some(&mut Prewarm::new(txs, order)), publish, BENEFICIARY);
        let expected = execute(txs, None, false, BENEFICIARY);
        assert_eq!(reused.receipts, expected.receipts);
        assert_eq!(reused.bundle, expected.bundle);
        assert_eq!(reused.changes, expected.changes);
        reused.outcomes
    }

    /// Executes `txs` as pool transactions on top of the parent state, as by the txpool prewarm
    /// worker, in a block environment that differs from the actual one in the timestamp and
    /// prevrandao.
    fn pool_results(
        txs: &[Recovered<TransactionSigned>],
    ) -> PoolResults<HaltReasonFor<EthEvmConfig>> {
        let mut evm_env = evm_env(true);
        evm_env.block_env.beneficiary = POOL_BENEFICIARY;
        evm_env.block_env.timestamp = U256::from(12);
        evm_env.block_env.prevrandao = Some(B256::repeat_byte(1));
        let mut evm = EthEvmConfig::mainnet().evm_with_env_and_inspector(
            RecordingDb::new(parent_state()),
            evm_env,
            BalanceTracker::default(),
        );
        let results = PoolResults::default();
        for tx in txs {
            let tx_env = EthEvmConfig::mainnet().tx_env(tx);
            let speculative = SpeculativeTx {
                caller: tx.signer(),
                nonce: tx_env.nonce,
                max_balance_spending: tx_env.max_balance_spending(),
            };
            evm.db_mut().start_recording_inner();
            evm.inspector_mut().clear();
            let result = evm.transact(tx_env).unwrap();
            let reads = evm.db_mut().take_reads().unwrap();
            results.store(*tx.tx_hash(), result, reads, evm.inspector(), speculative);
        }
        results
    }

    /// Asserts that reusing results of the `pool` transactions produces the same output as
    /// executing every transaction in a block with `beneficiary`, and returns the reuse outcomes.
    fn reuse_pool(
        txs: &[Recovered<TransactionSigned>],
        pool: &[Recovered<TransactionSigned>],
        beneficiary: Address,
    ) -> Vec<Result<(), ReuseMiss>> {
        let differs = BlockEnvFields { timestamp: true, prevrandao: true, ..Default::default() };
        let seeds = PoolSeeds::new(Arc::new(pool_results(pool)), beneficiary, differs);
        let mut prewarm = Prewarm::new(txs, &[]);
        for (index, tx) in txs.iter().enumerate() {
            prewarm.results.seed(index, tx.tx_hash(), &seeds);
        }
        let reused = execute(txs, Some(&mut prewarm), false, beneficiary);
        let expected = execute(txs, None, false, beneficiary);
        assert_eq!(reused.receipts, expected.receipts);
        assert_eq!(reused.bundle, expected.bundle);
        assert_eq!(reused.changes, expected.changes);
        reused.outcomes
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
    fn rebases_original_balances() {
        // The first transaction leaves the forwarder 5 wei more, and the second forwards 5 wei, so
        // its rebased balance equals its speculative balance before the second transaction.
        let txs = [tx(1, 0, FORWARDER, 10), tx(2, 0, FORWARDER, 0)];
        assert_eq!(reuse_in(&txs, &[1], false), [Err(ReuseMiss::Missing), Ok(())]);
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

    #[test]
    fn reuses_pool_results() {
        // The last transaction's caller was credited by the first one.
        let txs = [tx(1, 0, sender(2), 1), tx(3, 0, COUNTER, 1), tx(2, 0, GAS_LIMIT_READER, 1)];
        assert_eq!(reuse_pool(&txs, &txs, BENEFICIARY), [Ok(()), Ok(()), Ok(())]);
        // Transactions missing from the pool are executed.
        assert_eq!(
            reuse_pool(&txs, &txs[1..], BENEFICIARY),
            [Err(ReuseMiss::Missing), Ok(()), Ok(())]
        );
    }

    #[test]
    fn rejects_stale_pool_results() {
        // Test transactions are hashed without their sender.
        let txs = [tx(1, 0, COUNTER, 1), tx(2, 0, COUNTER, 2)];
        assert_eq!(reuse_pool(&txs, &txs, BENEFICIARY), [Ok(()), Err(ReuseMiss::Stale)]);
        // The beneficiary must exist to be credited.
        let missing = Address::repeat_byte(0xbf);
        assert_eq!(reuse_pool(&txs, &txs, missing), [Err(ReuseMiss::Stale), Err(ReuseMiss::Stale)]);
    }

    #[test]
    fn rejects_pool_results_observing_block_env() {
        let txs = [
            tx(1, 0, TIMESTAMP_READER, 1),
            tx(2, 0, BENEFICIARY_READER, 1),
            tx(3, 0, POOL_BENEFICIARY, 1),
            tx(4, 0, POOL_BENEFICIARY_CALLER, 1),
            tx(6, 0, BENEFICIARY, 1),
        ];
        assert_eq!(reuse_pool(&txs, &txs, BENEFICIARY), [Err(ReuseMiss::Missing); 5]);
    }

    #[test]
    fn bounds_pool_results() {
        let txs = [tx(1, 0, COUNTER, 1)];
        let results = pool_results(&txs);
        let tx = results.get(txs[0].tx_hash()).unwrap();
        let mut result = tx.result.clone();
        result.state.get_mut(&COUNTER).unwrap().storage.extend(
            (0..MAX_POOL_RESULT_ENTRIES as u64)
                .map(|slot| (U256::from(slot), EvmStorageSlot::default())),
        );
        let mut reads = tx.reads.clone();
        reads.accounts.push(AccountRead {
            address: POOL_BENEFICIARY,
            info: None,
            balance: BalanceDependency::None,
            predicted: false,
        });
        result.state.insert(POOL_BENEFICIARY, Account::default());
        let tx_env = EthEvmConfig::mainnet().tx_env(&txs[0]);
        let speculative = SpeculativeTx {
            caller: sender(1),
            nonce: 0,
            max_balance_spending: tx_env.max_balance_spending(),
        };
        assert!(!results.store(B256::ZERO, result, reads, &BalanceTracker::default(), speculative));
        assert!(results.get(&B256::ZERO).is_none());
    }

    #[test]
    fn rejects_invalid_transactions() {
        assert_eq!(reusable(&[tx(1, 0, NOOP, 0)]), Ok(()));
        // The caller cannot pay for the value.
        assert_eq!(reusable(&[tx(1, 0, NOOP, 10u64.pow(19))]), Err(ReuseMiss::Stale));
        // The caller's nonce differs.
        assert_eq!(reusable(&[tx(1, 1, NOOP, 0)]), Err(ReuseMiss::Stale));
        // The caller's nonce cannot be incremented.
        assert_eq!(reusable(&[tx(EXHAUSTED_SENDER, u64::MAX, NOOP, 0)]), Err(ReuseMiss::Stale));
        // Speculative execution fails.
        assert_eq!(
            reusable(&[tx_with_gas_limit(1, 0, NOOP, 0, 40_000_000)]),
            Err(ReuseMiss::Stale)
        );
        // The most the caller may spend overflows, though the effective fee does not.
        let overflowing = Transaction::Eip1559(TxEip1559 {
            chain_id: 1,
            max_fee_per_gas: u128::MAX,
            gas_limit: 100_000,
            to: TxKind::Call(NOOP),
            ..Default::default()
        });
        let overflowing = Recovered::new_unchecked(
            TransactionSigned::new_unhashed(overflowing, Signature::test_signature()),
            sender(1),
        );
        assert_eq!(reusable(&[overflowing]), Err(ReuseMiss::Stale));
    }

    #[test]
    fn keeps_stale_results_unchanged() {
        let txs = [tx(1, 0, NOOP, 0)];
        let prewarm = Prewarm::new(&txs, &[0]);
        // The caller's balance can be rebased, but the callee changed.
        let mut db = parent_state();
        db.insert_account_info(
            sender(1),
            AccountInfo { balance: U256::from(2 * 10u64.pow(18)), ..Default::default() },
        );
        let mut noop = db.basic(NOOP).unwrap().unwrap();
        noop.nonce = 1;
        db.insert_account_info(NOOP, noop);
        let mut stale = State::builder().with_database(db).build();
        assert_eq!(
            prewarm.results.take_valid(0, &mut stale).unwrap().map(drop),
            Err(ReuseMiss::Stale)
        );

        let mut state = State::builder().with_database(parent_state()).build();
        let result = prewarm.results.take_valid(0, &mut state).unwrap().unwrap();
        let fee = U256::from(21_000 * GAS_PRICE);
        assert_eq!(result.state[&sender(1)].info.balance, U256::from(10u64.pow(18)) - fee);
    }

    #[test]
    fn keeps_results_on_database_errors() {
        let txs = [tx(1, 0, NOOP, 0)];
        let prewarm = Prewarm::new(&txs, &[0]);
        assert!(matches!(
            prewarm.results.take_valid(0, &mut FailingDb),
            Err(ProviderError::BestBlockNotFound)
        ));
        let mut state = State::builder().with_database(parent_state()).build();
        assert_eq!(prewarm.results.take_valid(0, &mut state).unwrap().map(drop), Ok(()));
    }

    #[test]
    fn rejects_unknown_code() {
        assert_eq!(reuse_in(&[tx(1, 0, UNKNOWN_CODE, 0)], &[0], false), [Err(ReuseMiss::Stale)]);
    }

    #[test]
    fn rejects_rebasing_unbounded_or_zero_balances() {
        let rebased = [tx(1, 0, sender(4), 1), tx(2, 0, sender(4), 1)];
        assert_eq!(reuse_in(&rebased, &[1], false), [Err(ReuseMiss::Missing), Ok(())]);
        for txs in [
            [tx(1, 0, RICH, 1), tx(2, 0, RICH, 1)],
            [tx(1, 0, ZERO_BALANCE, 1), tx(2, 0, ZERO_BALANCE, 1)],
            // Any unbounded balance read makes every balance exact.
            [tx(1, 0, sender(4), 1), tx(4, 0, RICH, 1)],
        ] {
            assert_eq!(
                reuse_in(&txs, &[1], false),
                [Err(ReuseMiss::Missing), Err(ReuseMiss::Stale)]
            );
        }
    }
}

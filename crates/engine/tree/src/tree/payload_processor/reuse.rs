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

use alloy_consensus::{InMemorySize, Transaction};
use alloy_primitives::{
    address,
    map::{B256Map, DefaultHashBuilder, Entry},
    Address, B256, KECCAK256_EMPTY, U256,
};
use dashmap::DashMap;
use derive_where::derive_where;
use evm2::{
    bytecode::Bytecode,
    evm::{
        AccountChanges, AccountInfo, Database, Db, DbResult, DynDatabase, PendingState, StorageSlot,
    },
    interpreter::{op, InstrStop, Interpreter, Message, MessageKind, MessageResult},
    utils::word_to_address,
    EvmFeatures, EvmTypes, Inspector, TxResultWithState, Version,
};
use parking_lot::{Mutex, RwLock};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

/// Balances above this are compared exactly, so that applying balance changes cannot overflow.
const MAX_REBASED_BALANCE: U256 = U256::from_limbs([u64::MAX, u64::MAX, 0, 0]);

/// Beneficiary of speculative pool transaction executions.
///
/// Results of transactions that access it are discarded, so its value only affects how many are.
pub(crate) const POOL_BENEFICIARY: Address = address!("0x00000000000000000000000000000000000beef0");

/// Maximum estimated number of bytes held by [`PoolResults`].
const MAX_POOL_RESULT_BYTES: usize = 64 << 20;

/// Returns whether blocks executed with `cfg_env` may reuse prewarm results, which excludes
/// EIP-8037 multidimensional block gas.
pub(crate) const fn cfg_reuses_prewarm_results(version: &Version) -> bool {
    !version.feature(EvmFeatures::EIP8037)
}

/// Speculative prewarm results of a block's transactions, indexed by their position in the block.
#[derive_where(Debug)]
pub(crate) struct PrewarmResults<T: EvmTypes> {
    slots: Box<[Mutex<ResultSlot<T>>]>,
    /// Writes predicted by the stored results.
    predicted: Arc<PredictedState>,
}

/// Prewarm state of a transaction in [`PrewarmResults`].
#[derive_where(Debug)]
enum ResultSlot<T: EvmTypes> {
    /// Prewarming has not executed the transaction.
    Empty,
    /// A result that may be reused.
    Ready(Box<PrewarmedTx<T>>),
    /// A result that cannot be reused, with the reads it depends on.
    Unusable(ReadSet),
    /// Block execution executed the transaction or reused its result, which are published as
    /// its actual writes.
    Executed,
}

impl<T: EvmTypes> PrewarmResults<T> {
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
        result: TxResultWithState<T>,
        reads: ReadSet,
        balances: &mut BalanceTracker,
        tx: SpeculativeTx,
    ) {
        match reads.finish(balances, tx) {
            Ok(reads) => self.set(index, |predicted| {
                predicted.publish(index, &result.pending_state);
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
        seeds: &PoolSeeds<T>,
    ) -> Option<Arc<PoolTx<T>>>
    where
        T::TxResultExt: Clone,
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
        self.predicted.publish(index, &tx.result.pending_state);
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
    pub(crate) fn publish_executed(&self, index: usize, state: &PendingState) {
        let Some(slot) = self.slots.get(index) else { return };
        let mut slot = slot.lock();
        if let ResultSlot::Ready(previous) = core::mem::replace(&mut *slot, ResultSlot::Executed) {
            self.predicted.retract(index, &previous.result.pending_state);
        }
        self.predicted.publish(index, state);
        self.predicted.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Replaces the slot of the transaction at `index`, retracting the writes of its previous
    /// result, unless block execution already published its actual writes.
    fn set(&self, index: usize, f: impl FnOnce(&PredictedState) -> ResultSlot<T>) {
        let Some(slot) = self.slots.get(index) else { return };
        let mut slot = slot.lock();
        match &*slot {
            ResultSlot::Ready(previous) => {
                self.predicted.retract(index, &previous.result.pending_state)
            }
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
    pub(crate) fn take_valid<DB: DynDatabase + ?Sized>(
        &self,
        index: usize,
        db: &mut DB,
    ) -> Result<Result<TxResultWithState<T>, ReuseMiss>, evm2::DatabaseError> {
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
            self.predicted.publish_accounts(index, &tx.result.pending_state);
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
#[derive_where(Debug)]
struct PrewarmedTx<T: EvmTypes> {
    result: TxResultWithState<T>,
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

impl<T: EvmTypes> PrewarmedTx<T> {
    /// Validates recorded reads, then applies balance changes to the current state.
    fn validate<DB: DynDatabase + ?Sized>(&mut self, db: &mut DB) -> DbResult<Option<bool>> {
        for read in &self.reads.storage {
            if db.get_storage(&read.address, &read.slot)? != read.value {
                return Ok(None)
            }
        }
        let mut rebased = Vec::new();
        for read in &self.reads.accounts {
            let current = db.get_account(&read.address)?;
            match read.compare(current.as_ref()) {
                AccountMatch::Same => {}
                AccountMatch::Rebase { speculative, current } => {
                    if let Some(account) = self
                        .result
                        .pending_state
                        .account_changes()
                        .find(|a| a.address == read.address)
                    {
                        // Replacing metadata must not erase creation, destruction or storage-wipe
                        // flags.
                        if account.created || account.selfdestructed || account.storage.wiped {
                            return Ok(None)
                        }
                        let Some(mut info) = account.current.cloned() else { return Ok(None) };
                        let Some(balance) = info
                            .balance
                            .checked_add(current)
                            .and_then(|b| b.checked_sub(speculative))
                        else {
                            return Ok(None)
                        };
                        info.balance = balance;
                        let mut original = account.original.cloned();
                        if let Some(original) = &mut original {
                            original.balance = current;
                        }
                        rebased.push((read.address, original, Some(info)));
                    }
                }
                AccountMatch::Differs => return Ok(None),
            }
        }
        if let Some(credit) = &self.credit {
            let Some(original) = db.get_account(&credit.beneficiary)? else { return Ok(None) };
            let mut current = original.clone();
            let Some(balance) = current.balance.checked_add(credit.amount) else { return Ok(None) };
            current.balance = balance;
            // Execution touches the beneficiary even for a zero credit, which deletes it if it is
            // empty (EIP-161). Detached state cannot express that, so execute normally.
            if current.is_empty() {
                return Ok(None)
            }
            // The synthetic beneficiary was not observed. Preserve its current canonical cache
            // value when committing the detached result's loaded-but-unchanged entries.
            let synthetic = db.get_account(&POOL_BENEFICIARY)?;
            rebased.push((POOL_BENEFICIARY, synthetic.clone(), synthetic));
            rebased.push((credit.beneficiary, Some(original), Some(current)));
        }
        let changed = !rebased.is_empty();
        for (address, original, current) in rebased {
            self.result.pending_state.insert_account(address, original, current);
        }
        Ok(Some(changed))
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
            (AccountInfo { balance: speculative.balance, ..without_code(current) }) !=
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
    slot: U256,
    value: U256,
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
    fn finish(mut self, balances: &mut BalanceTracker, tx: SpeculativeTx) -> Result<Self, Self> {
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
        let Some(max_balance_spending) = max_balance_spending else { return Err(self) };
        // Transactions at the nonce limit cannot be reused, even with validation disabled.
        if self.unknown_code ||
            balances.read_failed ||
            nonce == u64::MAX ||
            caller_nonce != nonce ||
            caller_balance < max_balance_spending
        {
            return Err(self)
        }

        // Rebasing only bounded balances keeps every balance during execution far from overflow,
        // so no value transfer can overflow in one execution but not the other.
        let bounded = self
            .accounts
            .iter()
            .all(|read| read.info.as_ref().is_none_or(|info| info.balance <= MAX_REBASED_BALANCE));
        let paid = caller_balance - max_balance_spending;
        // Sorted, so that each account is looked up in logarithmic time and the smallest balance
        // a sender had left comes first.
        let BalanceTracker { observed, senders, .. } = balances;
        observed.sort_unstable();
        senders.sort_unstable();
        for read in &mut self.accounts {
            // A sender needs enough balance to leave the same smallest balance, which is the
            // balance after paying for gas for the caller.
            let sent = senders
                .get(senders.partition_point(|(address, _)| *address < read.address))
                .filter(|(address, _)| *address == read.address)
                .map(|(_, left)| *left);
            let left = sent.into_iter().chain((read.address == caller).then_some(paid)).min();
            read.balance = if !bounded || observed.binary_search(&read.address).is_ok() {
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
    pub(crate) max_balance_spending: Option<U256>,
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
    storage: DashMap<(Address, U256), Writes<U256>, DefaultHashBuilder>,
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
    fn storage(&self, address: Address, slot: U256, index: usize) -> Option<U256> {
        let cleared = self.cleared.get(&address).and_then(|w| latest(&w, index).map(|(i, _)| *i));
        let written = self.storage.get(&(address, slot)).and_then(|w| latest(&w, index).copied());
        match (written, cleared) {
            (Some((written, value)), Some(cleared)) if written >= cleared => Some(value),
            (_, Some(_)) => Some(U256::ZERO),
            (written, None) => written.map(|(_, value)| value),
        }
    }

    /// Returns predicted bytecode by its hash.
    fn code(&self, code_hash: B256) -> Option<Bytecode> {
        self.code.get(&code_hash).map(|code| code.clone())
    }

    /// Publishes the transaction's writes as predictions for later transactions.
    fn publish(&self, index: usize, state: &PendingState) {
        for account in state.account_changes() {
            if let Some((hash, code)) = account.code {
                self.code.insert(hash, code.clone());
            }
        }
        self.update(index, state.account_changes(), true);
    }

    /// Republishes rebased account balances.
    fn publish_accounts(&self, index: usize, state: &PendingState) {
        self.update(index, state.account_changes(), true);
    }

    /// Retracts the transaction's predicted writes.
    fn retract(&self, index: usize, state: &PendingState) {
        self.update(index, state.account_changes(), false);
    }

    /// Publishes or retracts each changed account and slot.
    fn update<'a>(
        &self,
        index: usize,
        state: impl IntoIterator<Item = AccountChanges<'a>>,
        publish: bool,
    ) {
        for account in state {
            if !account.is_changed() {
                continue
            }
            let address = account.address;
            if account.storage.wiped || account.created || account.current.is_none() {
                write(&mut self.cleared.entry(address).or_default(), index, publish.then_some(()));
            }
            write(
                &mut self.accounts.entry(address).or_default(),
                index,
                publish.then(|| account.current.map(without_code)),
            );
            if account.current.is_none() {
                continue
            }
            for (&slot, value) in &account.storage.slots {
                if (account.storage.wiped && !value.value.current.is_zero()) ||
                    (!account.storage.wiped && value.value.original != value.value.current)
                {
                    write(
                        &mut self.storage.entry((address, slot)).or_default(),
                        index,
                        publish.then_some(value.value.current),
                    );
                }
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
    /// The beneficiary, observed by `COINBASE`, by accessing its account, or possibly by running
    /// out of gas.
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
#[derive_where(Debug)]
pub(crate) struct PoolResults<T: EvmTypes> {
    stored: RwLock<StoredPoolTxs<T>>,
}

impl<T: EvmTypes> Default for PoolResults<T> {
    fn default() -> Self {
        Self { stored: RwLock::new(StoredPoolTxs { txs: B256Map::default(), bytes: 0 }) }
    }
}

/// Results of pool transactions stored in [`PoolResults`].
#[derive_where(Debug)]
struct StoredPoolTxs<T: EvmTypes> {
    txs: B256Map<Arc<PoolTx<T>>>,
    /// Estimated number of bytes held by `txs`.
    bytes: usize,
}

impl<T: EvmTypes> PoolResults<T> {
    /// Stores the result of the pool transaction with `hash` if [`ReadSet::finish`] accepts its
    /// reads, unless it may have accessed the beneficiary, a result is already stored for `hash`,
    /// or the stored results would hold more than [`MAX_POOL_RESULT_BYTES`].
    ///
    /// Returns whether the result was stored.
    pub(crate) fn store(
        &self,
        hash: B256,
        mut result: TxResultWithState<T>,
        reads: ReadSet,
        balances: &mut BalanceTracker,
        tx: SpeculativeTx,
    ) -> bool {
        if balances.block_env.beneficiary {
            return false
        }
        let Ok(mut reads) = reads.finish(balances, tx) else { return false };

        // The beneficiary was only loaded to credit fees, which are credited to the actual
        // beneficiary instead. Crediting fees always loads and touches it, even for zero fees.
        let Some(beneficiary) = result
            .pending_state
            .account_changes()
            .find(|account| account.address == POOL_BENEFICIARY)
        else {
            return false
        };
        // A zero fee can leave an empty beneficiary that EIP-161 clears during finalization.
        let balance = beneficiary.current.map_or(U256::ZERO, |info| info.balance);
        let Some(index) = reads.position(POOL_BENEFICIARY) else { return false };
        let original = reads.accounts.remove(index).info;
        let speculative = original.as_ref().map_or(U256::ZERO, |info| info.balance);
        if speculative > MAX_REBASED_BALANCE {
            return false
        }
        let Some(credit) = balance.checked_sub(speculative) else { return false };
        result.pending_state.insert_account(POOL_BENEFICIARY, original.clone(), original);

        let bytes = retained_bytes(&result, &reads);
        let mut stored = self.stored.write();
        let total = stored.bytes + bytes;
        if total > MAX_POOL_RESULT_BYTES {
            return false
        }
        let Entry::Vacant(entry) = stored.txs.entry(hash) else { return false };
        entry.insert(Arc::new(PoolTx { result, reads, credit }));
        stored.bytes = total;
        true
    }

    /// Returns the result of the pool transaction with `hash`.
    pub(crate) fn get(&self, hash: &B256) -> Option<Arc<PoolTx<T>>> {
        self.stored.read().txs.get(hash).cloned()
    }

    /// Returns whether no result is stored.
    pub(crate) fn is_empty(&self) -> bool {
        self.stored.read().txs.is_empty()
    }
}

/// Returns the estimated number of bytes that storing `result` and `reads` holds.
fn retained_bytes<T: EvmTypes>(result: &TxResultWithState<T>, reads: &ReadSet) -> usize {
    let accounts = result.pending_state.account_changes().map(|account| {
        size_of::<AccountInfo>() * 2 +
            size_of::<Address>() +
            account.storage.slots.capacity() * size_of::<(U256, StorageSlot)>() +
            account.code.map_or(0, |(_, code)| code.len())
    });
    reads.accounts.capacity() * size_of::<AccountRead>() +
        reads.storage.capacity() * size_of::<StorageRead>() +
        result.result.output.len() +
        result.result.logs.iter().map(InMemorySize::size).sum::<usize>() +
        accounts.sum::<usize>()
}

/// A speculative result of a pool transaction without its fee credit to the beneficiary.
#[derive_where(Debug)]
pub(crate) struct PoolTx<T: EvmTypes> {
    result: TxResultWithState<T>,
    reads: ReadSet,
    /// The fee credit to the beneficiary.
    credit: U256,
}

impl<T: EvmTypes> PoolTx<T> {
    /// Returns the state changes of the transaction.
    pub(super) const fn state(&self) -> &PendingState {
        &self.result.pending_state
    }

    /// Loads every read of the transaction from `db`.
    pub(super) fn load_reads<DB: DynDatabase + ?Sized>(
        &self,
        db: &mut DB,
    ) -> Result<(), evm2::DatabaseError> {
        for read in &self.reads.accounts {
            db.get_account(&read.address)?;
        }
        for read in &self.reads.storage {
            db.get_storage(&read.address, &read.slot)?;
        }
        Ok(())
    }
}

/// Results of pool transactions that may seed the [`PrewarmResults`] of a block on top of the
/// parent they were executed on.
#[derive_where(Debug, Clone)]
pub(crate) struct PoolSeeds<T: EvmTypes> {
    results: Arc<PoolResults<T>>,
    /// The beneficiary of the block.
    beneficiary: Address,
    /// Block environment fields that differ from the predicted environment.
    differs: BlockEnvFields,
}

impl<T: EvmTypes> PoolSeeds<T> {
    /// Creates seeds for a block with `beneficiary` whose environment differs from the predicted
    /// environment of `results` in `differs`.
    pub(crate) const fn new(
        results: Arc<PoolResults<T>>,
        beneficiary: Address,
        differs: BlockEnvFields,
    ) -> Self {
        Self { results, beneficiary, differs }
    }
}

/// Database wrapper that records reads for speculative execution.
#[derive(Debug)]
pub(crate) struct RecordingDb<DB: Database> {
    inner: Db<DB>,
    recording: RecordingHandle,
}

/// Per-worker recording state shared with its EVM and inspector.
#[derive(Debug, Default, Clone)]
pub(crate) struct RecordingHandle(Rc<RefCell<RecordingState>>);

#[derive(Debug, Default)]
struct RecordingState {
    reads: Option<ReadSet>,
    predicted: Option<(Arc<PredictedState>, usize)>,
}

impl RecordingHandle {
    /// Begins recording against earlier predicted writes, or just the parent state.
    pub(super) fn start(&self, predicted: Option<(Arc<PredictedState>, usize)>) {
        *self.0.borrow_mut() = RecordingState { reads: Some(ReadSet::default()), predicted };
    }

    /// Begins recording a pool transaction against its parent state.
    pub(crate) fn start_recording_inner(&self) {
        self.start(None);
    }

    /// Stops recording and returns the transaction's reads.
    pub(crate) fn take_reads(&self) -> Option<ReadSet> {
        let mut state = self.0.borrow_mut();
        state.predicted = None;
        state.reads.take()
    }
}

impl<DB: Database> RecordingDb<DB> {
    /// Wraps a database with the worker's recording state.
    pub(crate) const fn new(inner: DB, recording: RecordingHandle) -> Self {
        Self { inner: Db::new(inner), recording }
    }
}

impl<DB: Database> Database for RecordingDb<DB> {
    type Error = evm2::DatabaseError;

    fn is_fatal(error: &Self::Error) -> bool {
        error.is_fatal()
    }

    fn get_account(&mut self, address: &Address) -> DbResult<Option<AccountInfo>> {
        let mut state = self.recording.0.borrow_mut();
        let predicted =
            state.predicted.as_ref().and_then(|(state, index)| state.account(*address, *index));
        let is_predicted = predicted.is_some();
        let account = match predicted {
            Some(account) => account,
            None => self.inner.get_account(address)?,
        };
        if let Some(reads) = &mut state.reads {
            reads.accounts.push(AccountRead {
                address: *address,
                info: account.as_ref().map(without_code),
                balance: BalanceDependency::None,
                predicted: is_predicted,
            });
        }
        Ok(account)
    }

    fn get_code_by_hash(&mut self, hash: &B256) -> DbResult<Bytecode> {
        let mut state = self.recording.0.borrow_mut();
        if let Some(code) = state.predicted.as_ref().and_then(|(state, _)| state.code(*hash)) {
            return Ok(code)
        }
        let code = self.inner.get_code_by_hash(hash)?;
        if code.is_empty() &&
            *hash != KECCAK256_EMPTY &&
            let Some(reads) = &mut state.reads
        {
            reads.unknown_code = true;
        }
        Ok(code)
    }

    fn get_storage(&mut self, address: &Address, key: &U256) -> DbResult<U256> {
        let mut state = self.recording.0.borrow_mut();
        let predicted = state
            .predicted
            .as_ref()
            .and_then(|(state, index)| state.storage(*address, *key, *index));
        let is_predicted = predicted.is_some();
        let value = match predicted {
            Some(value) => value,
            None => self.inner.get_storage(address, key)?,
        };
        if let Some(reads) = &mut state.reads {
            reads.storage.push(StorageRead {
                address: *address,
                slot: *key,
                value,
                predicted: is_predicted,
            });
        }
        Ok(value)
    }

    fn get_block_hash(&mut self, number: &U256) -> DbResult<B256> {
        self.inner.get_block_hash(number)
    }
}

/// Copies account metadata without retaining the bytecode allocation.
fn without_code(info: &AccountInfo) -> AccountInfo {
    let mut info = info.clone();
    info.code = None;
    info
}

impl SpeculativeTx {
    /// Captures the nonce and maximum fee and value debit checked by canonical execution.
    pub(crate) fn new(caller: Address, tx: &impl Transaction) -> Self {
        let max_balance_spending = U256::from(tx.gas_limit())
            .checked_mul(U256::from(tx.max_fee_per_gas()))
            .and_then(|gas| gas.checked_add(tx.value()))
            .and_then(|gas| {
                U256::from(tx.blob_gas_used().unwrap_or_default())
                    .checked_mul(U256::from(tx.max_fee_per_blob_gas().unwrap_or_default()))
                    .and_then(|blob| gas.checked_add(blob))
            });
        Self { caller, nonce: tx.nonce(), max_balance_spending }
    }
}

/// Inspector that tracks which balances and block environment fields a transaction observes and
/// which accounts send value.
#[derive(Debug, Default)]
pub(crate) struct BalanceTracker {
    recording: RecordingHandle,
    read_failed: bool,
    /// Accounts whose balance was observed.
    observed: Vec<Address>,
    /// Accounts that sent value, with the balance they had left after each send.
    senders: Vec<(Address, U256)>,
    /// Block environment fields that were observed.
    block_env: BlockEnvFields,
}

impl BalanceTracker {
    /// Creates an inspector sharing the worker's recorded database reads.
    pub(crate) fn new(recording: RecordingHandle) -> Self {
        Self { recording, ..Self::default() }
    }

    fn frame_end<T: EvmTypes>(
        &mut self,
        interp: &mut Interpreter<'_, '_, T>,
        message: &Message<T>,
        result: &MessageResult<T>,
    ) {
        let beneficiary = interp.host().block().beneficiary;
        self.block_env.beneficiary |=
            result.stop.is_out_of_gas() ||
                (message.depth == 0 &&
                    self.recording.0.borrow_mut().reads.as_ref().is_some_and(|r| {
                        r.accounts.iter().any(|a| a.address == beneficiary)
                    }));
    }

    fn send<T: EvmTypes>(
        &mut self,
        interp: &mut Interpreter<'_, '_, T>,
        sender: Address,
        value: U256,
    ) {
        let left = match interp.host().state_mut().account_info_untracked(&sender) {
            Ok(account) => account.and_then(|account| account.balance.checked_sub(value)),
            Err(_) => {
                // A failed observer read cannot justify reusing the speculative result.
                self.read_failed = true;
                None
            }
        };
        match left {
            Some(left) => self.senders.push((sender, left)),
            None => self.observed.push(sender),
        }
    }
}

impl<T: EvmTypes> Inspector<T> for BalanceTracker {
    fn step(&mut self, interp: &mut Interpreter<'_, '_, T>) {
        let address = match interp.opcode() {
            op::BALANCE => {
                let Some(word) = interp.stack().peek(0) else { return };
                word_to_address(word)
            }
            op::SELFBALANCE | op::SELFDESTRUCT => interp.message().destination,
            op::COINBASE => {
                self.block_env.beneficiary = true;
                return
            }
            op::TIMESTAMP => {
                self.block_env.timestamp = true;
                return
            }
            op::DIFFICULTY => {
                self.block_env.prevrandao = true;
                return
            }
            op::GASLIMIT => {
                self.block_env.gas_limit = true;
                return
            }
            _ => return,
        };
        if self.observed.last() != Some(&address) {
            self.observed.push(address);
        }
    }

    fn call(
        &mut self,
        interp: &mut Interpreter<'_, '_, T>,
        message: &mut Message<T>,
    ) -> Option<MessageResult<T>> {
        if matches!(message.kind, MessageKind::Call | MessageKind::CallCode) &&
            !message.value.is_zero()
        {
            self.send(interp, message.caller, message.value);
        }
        None
    }

    fn call_end(
        &mut self,
        interp: &mut Interpreter<'_, '_, T>,
        message: &Message<T>,
        result: &mut MessageResult<T>,
    ) {
        if result.stop == InstrStop::OutOfFunds {
            self.observed.push(message.caller);
        }
        self.frame_end(interp, message, result);
    }

    fn create(
        &mut self,
        interp: &mut Interpreter<'_, '_, T>,
        message: &mut Message<T>,
    ) -> Option<MessageResult<T>> {
        if !message.value.is_zero() {
            self.send(interp, message.caller, message.value);
        }
        None
    }

    fn create_end(
        &mut self,
        interp: &mut Interpreter<'_, '_, T>,
        message: &Message<T>,
        result: &mut MessageResult<T>,
    ) {
        if result.stop == InstrStop::OutOfFunds {
            self.observed.push(message.caller);
        }
        self.frame_end(interp, message, result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{transaction::Recovered, TxEip1559, TxLegacy};
    use alloy_primitives::{bytes, Bytes, Signature, TxKind};
    use evm2::{
        evm::{CacheDB, EmptyDB},
        BaseEvmTypes, SpecId,
    };
    use reth_errors::ProviderError;
    use reth_ethereum_primitives::{Receipt, Transaction, TransactionSigned};
    use reth_evm::{BlockExecutor, BlockExecutorFactory, ConfigureEvm, Evm, EvmFor};
    use reth_evm_ethereum::{EthBlockExecutionCtx, EthEvmConfig, EthEvmEnv};
    use revm::database::BundleState;

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
    /// Reads the balance of [`BENEFICIARY`] by its address.
    const BENEFICIARY_BALANCE_BY_ADDRESS: Address = Address::repeat_byte(0xcc);
    /// Stores the prevrandao in slot 0.
    const PREVRANDAO_READER: Address = Address::repeat_byte(0xcd);
    /// Has the largest balance.
    const SATURATED: Address = Address::repeat_byte(0xce);
    /// Index of a sender with the largest nonce.
    const EXHAUSTED_SENDER: u8 = 5;
    /// Selfdestructs to the noop contract.
    const SELFDESTRUCTER: Address = Address::repeat_byte(0xcf);
    /// Exists and is empty.
    const EMPTY: Address = Address::repeat_byte(0xd0);
    const GAS_PRICE: u128 = 2;

    type Results = PrewarmResults<BaseEvmTypes>;

    fn evm_env(prewarm: bool) -> EthEvmEnv {
        let mut env = EthEvmEnv::new(
            SpecId::CANCUN,
            evm2::env::BlockEnv::<BaseEvmTypes> {
                number: U256::from(1),
                beneficiary: BENEFICIARY,
                basefee: U256::ONE,
                gas_limit: U256::from(30_000_000),
                ..Default::default()
            },
            1,
        );
        if prewarm {
            env.version.features.remove(EvmFeatures::NONCE_CHECK | EvmFeatures::BALANCE_CHECK);
        }
        env
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
        tx_with_gas_price(from, nonce, to, value, gas_limit, GAS_PRICE)
    }

    fn tx_with_gas_price(
        from: u8,
        nonce: u64,
        to: Address,
        value: u64,
        gas_limit: u64,
        gas_price: u128,
    ) -> Recovered<TransactionSigned> {
        let tx = Transaction::Legacy(TxLegacy {
            chain_id: Some(1),
            nonce,
            gas_price,
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
        for i in [1, 2, 3, 4, 6, 7] {
            db.insert_account_info(
                &sender(i),
                AccountInfo { balance: U256::from(10u64.pow(18)), ..Default::default() },
            );
        }
        db.insert_account_info(
            &sender(EXHAUSTED_SENDER),
            AccountInfo {
                balance: U256::from(10u64.pow(18)),
                nonce: u64::MAX,
                ..Default::default()
            },
        );
        db.insert_account_info(
            &BENEFICIARY,
            AccountInfo { balance: U256::from(1), ..Default::default() },
        );
        db.insert_account_info(
            &RICH,
            AccountInfo { balance: U256::MAX >> 1, ..Default::default() },
        );
        db.insert_account_info(
            &SATURATED,
            AccountInfo { balance: U256::MAX, ..Default::default() },
        );
        db.insert_account_info(
            &sender(1).create(0),
            AccountInfo { balance: U256::from(12), ..Default::default() },
        );
        db.insert_account_info(&ZERO_BALANCE, AccountInfo { nonce: 1, ..Default::default() });
        db.insert_account_info(&EMPTY, AccountInfo::default());
        db.insert_account_info(
            &UNKNOWN_CODE,
            AccountInfo { code_hash: B256::repeat_byte(0xab), code: None, ..Default::default() },
        );
        for (address, code) in [
            (NOOP, bytes!("00")),
            (SELFDESTRUCTER, bytes!("73c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0ff")),
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
            (
                BENEFICIARY_BALANCE_BY_ADDRESS,
                bytes!("73bebebebebebebebebebebebebebebebebebebebe3100"),
            ),
            (PREVRANDAO_READER, bytes!("445f5500")),
        ] {
            let code = Bytecode::new_raw(code);
            db.insert_account_info(
                &address,
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
        evm: EvmFor<'static, EthEvmConfig>,
        recording: RecordingHandle,
    }

    impl<'a> Prewarm<'a> {
        /// Executes the transactions at the indices in `order`, then refreshes stale results
        /// among them in block order.
        fn new(txs: &'a [Recovered<TransactionSigned>], order: &[usize]) -> Self {
            let recording = RecordingHandle::default();
            let evm = EthEvmConfig::mainnet()
                .evm_with_env(RecordingDb::new(parent_state(), recording.clone()), evm_env(true));
            let mut prewarm = Self { txs, results: Results::new(txs.len()), evm, recording };
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
            let tx_env = EthEvmConfig::mainnet().tx_env(tx.clone());
            let speculative = SpeculativeTx::new(tx.signer(), tx.inner());
            drop(self.evm.take_state_cache());
            self.recording.start(Some((Arc::clone(self.results.predicted()), index)));
            let result = self
                .evm
                .transact_with_inspector(&tx_env, BalanceTracker::new(self.recording.clone()));
            let reads = self.recording.take_reads().unwrap();
            match result {
                Ok((mut balances, result)) => {
                    self.results.store(index, result, reads, &mut balances, speculative)
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
        let mut env = evm_env(false);
        env.block.beneficiary = beneficiary;
        let evm = config.evm_with_env(parent_state(), env);
        let ctx = EthBlockExecutionCtx {
            parent_hash: B256::ZERO,
            parent_beacon_block_root: None,
            ommers: &[],
            withdrawals: None,
            extra_data: Bytes::new(),
            tx_count_hint: None,
            slot_number: None,
        };
        let mut executor = config.block_executor_factory().create_executor(evm, ctx);
        let mut outcomes = Vec::new();
        let mut changes = Vec::new();
        for (index, tx) in txs.iter().enumerate() {
            let outcome = if let Some(prewarm) = prewarm.as_deref_mut() {
                prewarm.results.take_valid(index, executor.evm_mut().state_db_mut()).unwrap()
            } else {
                Err(ReuseMiss::Missing)
            };
            match outcome {
                Ok(result) => {
                    changes.push(changed_accounts(&result.pending_state));
                    executor
                        .commit_transaction(config.prewarm_tx_result(tx.inner(), result).unwrap())
                        .unwrap();
                    outcomes.push(Ok(()));
                }
                Err(miss) => {
                    let result = executor.execute_transaction_without_commit(tx.clone()).unwrap();
                    changes.push(changed_accounts(&result.result().pending_state));
                    if let Some(prewarm) = prewarm.as_deref_mut() &&
                        publish
                    {
                        prewarm.results.publish_executed(index, &result.result().pending_state);
                    }
                    executor.commit_transaction(result).unwrap();
                    if let Some(prewarm) = prewarm.as_deref_mut() &&
                        publish
                    {
                        prewarm.refresh_after(index);
                    }
                    outcomes.push(Err(miss));
                }
            }
        }
        let output = executor.finish().unwrap();
        Execution { receipts: output.result.receipts, bundle: output.state, changes, outcomes }
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
    fn changed_accounts(state: &PendingState) -> Vec<(Address, AccountInfo)> {
        let mut changed = state
            .account_changes()
            .filter(|account| account.is_info_changed())
            .filter_map(|account| account.current.map(|info| (account.address, without_code(info))))
            .collect::<Vec<_>>();
        changed.sort_unstable_by_key(|(address, _)| *address);
        changed
    }

    /// Database that fails every read.
    struct FailingDb;

    impl DynDatabase for FailingDb {
        fn get_account(&mut self, _: &Address) -> DbResult<Option<AccountInfo>> {
            Err(evm2::DatabaseError::new(ProviderError::BestBlockNotFound, true))
        }
        fn get_code_by_hash(&mut self, _: &B256) -> DbResult<Bytecode> {
            Err(evm2::DatabaseError::new(ProviderError::BestBlockNotFound, true))
        }
        fn get_storage(&mut self, _: &Address, _: &U256) -> DbResult<U256> {
            Err(evm2::DatabaseError::new(ProviderError::BestBlockNotFound, true))
        }
        fn get_block_hash(&mut self, _: &U256) -> DbResult<B256> {
            Err(evm2::DatabaseError::new(ProviderError::BestBlockNotFound, true))
        }
    }

    /// Returns whether the prewarm result of the first transaction can be reused, for
    /// transactions that block execution would reject.
    fn reusable(txs: &[Recovered<TransactionSigned>]) -> Result<(), ReuseMiss> {
        let prewarm = Prewarm::new(txs, &[0]);
        let mut state = parent_state();
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
    fn pool_results(txs: &[Recovered<TransactionSigned>]) -> PoolResults<BaseEvmTypes> {
        let mut env = evm_env(true);
        env.block.beneficiary = POOL_BENEFICIARY;
        env.block.timestamp = U256::from(12);
        env.block.prevrandao = U256::from_be_bytes([1; 32]);
        let recording = RecordingHandle::default();
        let mut evm = EthEvmConfig::mainnet()
            .evm_with_env(RecordingDb::new(parent_state(), recording.clone()), env);
        let results = PoolResults::default();
        for tx in txs {
            let tx_env = EthEvmConfig::mainnet().tx_env(tx.clone());
            let speculative = SpeculativeTx::new(tx.signer(), tx.inner());
            drop(evm.take_state_cache());
            recording.start_recording_inner();
            let (mut balances, result) = evm
                .transact_with_inspector(&tx_env, BalanceTracker::new(recording.clone()))
                .unwrap();
            let reads = recording.take_reads().unwrap();
            results.store(*tx.tx_hash(), result, reads, &mut balances, speculative);
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
        reuse_pool_with(txs, pool, beneficiary, differs)
    }

    /// Like [`reuse_pool`], for a block environment that differs from the predicted one in
    /// `differs`.
    fn reuse_pool_with(
        txs: &[Recovered<TransactionSigned>],
        pool: &[Recovered<TransactionSigned>],
        beneficiary: Address,
        differs: BlockEnvFields,
    ) -> Vec<Result<(), ReuseMiss>> {
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
    fn rejects_zero_credit_to_empty_beneficiary() {
        // Executing the credit deletes the empty beneficiary, which detached state cannot express.
        let txs = [tx_with_gas_price(1, 0, COUNTER, 1, 100_000, 1)];
        assert_eq!(reuse_pool(&txs, &txs, EMPTY), [Err(ReuseMiss::Stale)]);
    }

    #[test]
    fn rejects_pool_results_observing_block_env() {
        let txs = [
            tx(1, 0, TIMESTAMP_READER, 1),
            tx(2, 0, BENEFICIARY_READER, 1),
            tx(3, 0, POOL_BENEFICIARY, 1),
            tx(4, 0, POOL_BENEFICIARY_CALLER, 1),
            tx(6, 0, BENEFICIARY, 1),
            tx(7, 0, PREVRANDAO_READER, 1),
        ];
        assert_eq!(reuse_pool(&txs, &txs, BENEFICIARY), [Err(ReuseMiss::Missing); 6]);

        let txs = [tx(1, 0, GAS_LIMIT_READER, 1)];
        let differs = BlockEnvFields { gas_limit: true, ..Default::default() };
        assert_eq!(reuse_pool_with(&txs, &txs, BENEFICIARY, differs), [Err(ReuseMiss::Missing)]);
    }

    #[test]
    fn seeds_only_unexecuted_transactions() {
        let txs = [tx(1, 0, COUNTER, 1)];
        let differs = BlockEnvFields::default();
        let seeds = PoolSeeds::new(Arc::new(pool_results(&txs)), BENEFICIARY, differs);
        assert!(Prewarm::new(&txs, &[]).results.seed(0, txs[0].tx_hash(), &seeds).is_some());
        assert!(Prewarm::new(&txs, &[0]).results.seed(0, txs[0].tx_hash(), &seeds).is_none());
    }

    #[test]
    fn rejects_pool_results_overflowing_the_beneficiary() {
        let txs = [tx(1, 0, COUNTER, 1)];
        assert_eq!(reuse_pool(&txs, &txs, SATURATED), [Err(ReuseMiss::Stale)]);
    }

    /// Stores the recorded result of `tx` in `results` again under `hash`, after `modify` changed
    /// it and the read of the pool beneficiary.
    fn store_again(
        results: &PoolResults<BaseEvmTypes>,
        tx: &Recovered<TransactionSigned>,
        hash: B256,
        modify: impl FnOnce(&mut TxResultWithState<BaseEvmTypes>, &mut AccountRead),
    ) -> bool {
        let recorded = results.get(tx.tx_hash()).unwrap();
        let mut result = recorded.result.clone();
        result.pending_state.insert_account(POOL_BENEFICIARY, None, Some(AccountInfo::default()));
        let mut beneficiary = AccountRead {
            address: POOL_BENEFICIARY,
            info: None,
            balance: BalanceDependency::None,
            predicted: false,
        };
        modify(&mut result, &mut beneficiary);
        let mut reads = recorded.reads.clone();
        reads.accounts.push(beneficiary);
        let speculative = SpeculativeTx::new(tx.signer(), tx.inner());
        results.store(hash, result, reads, &mut BalanceTracker::default(), speculative)
    }

    #[test]
    fn bounds_pool_results() {
        let txs = [tx(1, 0, COUNTER, 1)];
        let results = pool_results(&txs);
        assert!(store_again(&results, &txs[0], B256::ZERO, |_, _| {}));
        // Deployed code counts towards the bound.
        assert!(!store_again(&results, &txs[0], B256::with_last_byte(1), |result, _| {
            let original = result.pending_state.account_info(&COUNTER).unwrap().clone();
            let current = original
                .clone()
                .with_code(Bytecode::new_raw(vec![0; MAX_POOL_RESULT_BYTES].into()));
            result.pending_state.insert_account(COUNTER, Some(original), Some(current));
        }));
    }

    #[test]
    fn rejects_recorded_pool_hashes() {
        let txs = [tx(1, 0, COUNTER, 1)];
        let results = pool_results(&txs);
        assert!(!store_again(&results, &txs[0], *txs[0].tx_hash(), |_, _| {}));
        assert!(store_again(&results, &txs[0], B256::ZERO, |_, _| {}));
    }

    #[test]
    fn rejects_saturated_pool_beneficiaries() {
        let txs = [tx(1, 0, COUNTER, 1)];
        let results = pool_results(&txs);
        // Crediting a saturated balance leaves it unchanged.
        assert!(!store_again(&results, &txs[0], B256::ZERO, |result, beneficiary| {
            beneficiary.info = Some(AccountInfo { balance: U256::MAX, ..Default::default() });
            result.pending_state.insert_account(POOL_BENEFICIARY, None, beneficiary.info.clone());
        }));
        assert!(store_again(&results, &txs[0], B256::ZERO, |_, _| {}));
    }

    #[test]
    fn rejects_pool_results_running_out_of_gas() {
        // Enough gas to read the balance of the warm beneficiary in the block, but not of the cold
        // one during speculation.
        let txs = [tx_with_gas_limit(1, 0, BENEFICIARY_BALANCE_BY_ADDRESS, 0, 21_500)];
        assert_eq!(reuse_pool(&txs, &txs, BENEFICIARY), [Err(ReuseMiss::Missing)]);
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
            &sender(1),
            AccountInfo { balance: U256::from(2 * 10u64.pow(18)), ..Default::default() },
        );
        let mut noop = DynDatabase::get_account(&mut db, &NOOP).unwrap().unwrap();
        noop.nonce = 1;
        db.insert_account_info(&NOOP, noop);
        let mut stale = db;
        assert_eq!(
            prewarm.results.take_valid(0, &mut stale).unwrap().map(drop),
            Err(ReuseMiss::Stale)
        );

        let mut state = parent_state();
        let result = prewarm.results.take_valid(0, &mut state).unwrap().unwrap();
        let fee = U256::from(21_000 * GAS_PRICE);
        assert_eq!(
            result.pending_state.account_info(&sender(1)).unwrap().balance,
            U256::from(10u64.pow(18)) - fee
        );
    }

    #[test]
    fn keeps_results_on_database_errors() {
        let txs = [tx(1, 0, NOOP, 0)];
        let prewarm = Prewarm::new(&txs, &[0]);
        assert!(matches!(
            prewarm.results.take_valid(0, &mut FailingDb),
            Err(err) if err.to_string() == ProviderError::BestBlockNotFound.to_string()
        ));
        let mut state = parent_state();
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

    #[test]
    fn preserves_account_lifecycle_and_rejects_rebasing_it() {
        let creation = Recovered::new_unchecked(
            TransactionSigned::new_unhashed(
                Transaction::Legacy(TxLegacy {
                    chain_id: Some(1),
                    gas_price: GAS_PRICE,
                    gas_limit: 100_000,
                    to: TxKind::Create,
                    input: bytes!("60015f5500"),
                    ..Default::default()
                }),
                Signature::test_signature(),
            ),
            sender(1),
        );
        for (transaction, address) in
            [(creation, sender(1).create(0)), (tx(1, 0, SELFDESTRUCTER, 0), SELFDESTRUCTER)]
        {
            let txs = [transaction];
            assert_eq!(reuse(&txs), [Ok(())]);
            let prewarm = Prewarm::new(&txs, &[0]);
            // An observed SELFDESTRUCT balance is exact; creation also cannot have its
            // lifecycle flags reset by metadata replacement during rebasing.
            let mut db = parent_state();
            let mut info = DynDatabase::get_account(&mut db, &address).unwrap().unwrap();
            info.balance += U256::ONE;
            db.insert_account_info(&address, info);
            assert_eq!(
                prewarm.results.take_valid(0, &mut db).unwrap().map(drop),
                Err(ReuseMiss::Stale)
            );
            let result = prewarm.results.take_valid(0, &mut parent_state()).unwrap().unwrap();
            let account =
                result.pending_state.account_changes().find(|a| a.address == address).unwrap();
            if address == sender(1).create(0) {
                assert!(account.created);
                assert_eq!(account.storage.slots[&U256::ZERO].value.current, U256::ONE);
            } else {
                // EIP-6780 retains pre-existing contracts after SELFDESTRUCT.
                assert_eq!(account.current.unwrap().balance, U256::ZERO);
            }
        }
    }

    #[test]
    fn rejects_results_after_failed_observer_reads() {
        let reads = ReadSet {
            accounts: vec![AccountRead {
                address: sender(1),
                info: Some(AccountInfo {
                    balance: U256::from(10u64.pow(18)),
                    ..Default::default()
                }),
                balance: BalanceDependency::None,
                predicted: false,
            }],
            ..Default::default()
        };
        let mut balances = BalanceTracker { read_failed: true, ..Default::default() };
        let tx = tx(1, 0, NOOP, 0);
        assert!(reads.finish(&mut balances, SpeculativeTx::new(tx.signer(), tx.inner())).is_err());
    }
}

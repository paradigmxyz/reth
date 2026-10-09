//! Validated reuse of block-local transaction prewarming results.
//!
//! Workers execute against the parent state with normal nonce and balance validation. Canonical
//! execution validates ready results' read preconditions before replaying committed storage
//! operations through the ordinary block executor. Results never cross payload boundaries.
//! Unobserved balance deltas are rebased; EVM balance reads and internal value-debit checks
//! remain exact dependencies. Caller funding checks conservatively require its parent balance.

use alloy_consensus::{transaction::TxHashRef, Transaction, TxReceipt};
use alloy_evm::{
    block::{BlockExecutor, TxResult},
    Evm,
};
use alloy_primitives::{
    map::{AddressMap, AddressSet, HashMap},
    Address, B256, U256,
};
use metrics::Counter;
use parking_lot::Mutex;
use reth_evm::{ConfigureEvm, HaltReasonFor, SpecFor, TxExecutionResultFor};
use reth_metrics::Metrics;
use reth_primitives_traits::TxTy;
use revm::{
    bytecode::{opcode, Bytecode},
    context::{
        result::{ExecutionResult, ResultAndState},
        Block, Cfg,
    },
    interpreter::{
        interpreter_types::{InputsTr, Jumps},
        CallInputs, CallOutcome, CreateInputs, CreateOutcome, Interpreter,
    },
    state::{AccountInfo, EvmState},
    Database, Inspector,
};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

const MAX_READS: usize = 8_192;
pub(super) const MAX_READY_RESULTS: usize = 256;
const MAX_REFRESHES: usize = 128;

/// A parent-state database that records every account and storage dependency.
#[derive(Debug)]
pub(super) struct RecordingDatabase<DB> {
    inner: DB,
    reads: ReadSet,
    beneficiary: Address,
    after_execution: Arc<AtomicBool>,
    actions: Rc<RefCell<StorageJournal>>,
    enabled: bool,
    storage_seed: StorageSeed,
}

impl<DB> RecordingDatabase<DB> {
    pub(super) fn new(inner: DB, beneficiary: Address, enabled: bool) -> Self {
        Self {
            inner,
            beneficiary,
            enabled,
            reads: ReadSet::default(),
            storage_seed: HashMap::default(),
            after_execution: Arc::new(AtomicBool::new(false)),
            actions: Rc::new(RefCell::new(StorageJournal::default())),
        }
    }

    pub(super) fn inspector(&self) -> HandoffInspector {
        HandoffInspector {
            after_execution: Arc::clone(&self.after_execution),
            actions: Rc::clone(&self.actions),
            checkpoints: Vec::new(),
            enabled: self.enabled,
        }
    }

    pub(super) fn reset(&mut self) {
        self.reads = ReadSet::default();
        self.storage_seed.clear();
        self.after_execution.store(false, Ordering::Relaxed);
        *self.actions.borrow_mut() = StorageJournal::default();
    }

    pub(super) fn set_recording(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.reset();
    }

    pub(super) fn take_reads(&mut self) -> ReadSet {
        let mut reads = std::mem::take(&mut self.reads);
        let actions = std::mem::take(&mut *self.actions.borrow_mut());
        reads.writes = actions.writes;
        reads.balance_reads = actions.balance_reads;
        reads.overflowed |= actions.overflowed;
        reads
    }

    /// Seeds a private retry database; shared parent-state caches are never changed.
    pub(super) fn seed_storage(&mut self, seed: StorageSeed) {
        self.storage_seed = seed;
    }

    fn can_record(&mut self) -> bool {
        if !self.enabled || self.reads.overflowed {
            return false
        }
        if self.reads.accounts.len() + self.reads.storage.len() + self.reads.block_hashes.len() >=
            MAX_READS
        {
            self.reads = ReadSet { overflowed: true, ..Default::default() };
            return false
        }
        true
    }
}

impl<DB: Database> Database for RecordingDatabase<DB> {
    type Error = DB::Error;

    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        let info = self.inner.basic(address)?;
        if self.can_record() {
            let fee_only =
                address == self.beneficiary && self.after_execution.load(Ordering::Relaxed);
            self.reads
                .accounts
                .entry(address)
                .or_insert_with(|| AccountRead { info: info.clone(), fee_only });
        }
        Ok(info)
    }

    fn code_by_hash(&mut self, hash: B256) -> Result<Bytecode, Self::Error> {
        self.inner.code_by_hash(hash)
    }

    fn storage(&mut self, address: Address, slot: U256) -> Result<U256, Self::Error> {
        let value = match self.storage_seed.get(&(address, slot)) {
            Some(value) => *value,
            None => self.inner.storage(address, slot)?,
        };
        if self.can_record() {
            self.reads.storage.entry((address, slot)).or_insert(value);
        }
        Ok(value)
    }

    fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
        let hash = self.inner.block_hash(number)?;
        if self.can_record() {
            self.reads.block_hashes.entry(number).or_insert(hash);
        }
        Ok(hash)
    }
}

/// Records persistent writes and balance observations, retaining reads across reverted frames.
#[derive(Debug)]
pub(super) struct HandoffInspector {
    after_execution: Arc<AtomicBool>,
    actions: Rc<RefCell<StorageJournal>>,
    checkpoints: Vec<FrameTrace>,
    enabled: bool,
}

impl HandoffInspector {
    pub(super) fn reset(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.checkpoints.clear();
        self.after_execution.store(false, Ordering::Relaxed);
    }

    fn enter(&mut self) {
        if !self.enabled {
            return
        }
        if let Some(parent) = self.checkpoints.last_mut() {
            if parent.has_code && !parent.stepped {
                self.actions.borrow_mut().overflowed = true;
            }
            // A parent can resume with a different execution backend after its child returns.
            // Require opcode inspection independently for every resumed execution segment.
            parent.stepped = false;
        }
        self.checkpoints.push(FrameTrace {
            writes: self.actions.borrow().writes.len(),
            has_code: false,
            stepped: false,
        });
        self.after_execution.store(false, Ordering::Relaxed);
    }

    fn exit(&mut self, success: bool) {
        if !self.enabled {
            return
        }
        if let Some(frame) = self.checkpoints.pop() {
            let mut actions = self.actions.borrow_mut();
            // JIT frames may forward call/log hooks without forwarding individual opcodes.
            // Never interpret absent balance observations as permission to rebase balances.
            actions.overflowed |= frame.has_code && !frame.stepped;
            if !success {
                actions.writes.truncate(frame.writes);
            }
        }
        if self.checkpoints.is_empty() {
            self.after_execution.store(true, Ordering::Relaxed);
        }
    }

    fn record_balance(&self, address: Address) {
        let mut actions = self.actions.borrow_mut();
        if actions.overflowed {
            return
        }
        if actions.balance_reads.len() >= MAX_READS {
            actions.balance_reads.clear();
            actions.overflowed = true;
            return
        }
        actions.balance_reads.insert(address);
    }
}

#[derive(Debug)]
struct FrameTrace {
    writes: usize,
    has_code: bool,
    stepped: bool,
}

impl<Ctx> Inspector<Ctx> for HandoffInspector {
    fn initialize_interp(&mut self, interp: &mut Interpreter, _ctx: &mut Ctx) {
        if self.enabled &&
            let Some(frame) = self.checkpoints.last_mut()
        {
            frame.has_code = !interp.bytecode.original_bytes().is_empty();
        }
    }

    fn step(&mut self, interp: &mut Interpreter, _ctx: &mut Ctx) {
        if !self.enabled {
            return
        }
        if let Some(frame) = self.checkpoints.last_mut() {
            frame.stepped = true;
        }
        match interp.bytecode.opcode() {
            opcode::BALANCE => {
                if let Ok(value) = interp.stack.peek(0) {
                    self.record_balance(Address::from_word(B256::from(value)));
                }
            }
            opcode::SELFBALANCE | opcode::SELFDESTRUCT => {
                self.record_balance(interp.input.target_address());
            }
            _ => {}
        }
        if self.enabled &&
            interp.bytecode.opcode() == opcode::SSTORE &&
            let Ok(slot) = interp.stack.peek(0) &&
            let Ok(value) = interp.stack.peek(1)
        {
            let mut actions = self.actions.borrow_mut();
            if actions.overflowed {
                return
            }
            if actions.writes.len() >= MAX_READS {
                actions.writes.clear();
                actions.overflowed = true;
                return
            }
            actions.writes.push(StorageWrite {
                address: interp.input.target_address(),
                slot,
                value,
            });
        }
    }

    fn call(&mut self, _ctx: &mut Ctx, inputs: &mut CallInputs) -> Option<CallOutcome> {
        if self.enabled && !self.checkpoints.is_empty() && !inputs.call_value().is_zero() {
            self.record_balance(inputs.caller);
        }
        self.enter();
        None
    }

    fn call_end(&mut self, _ctx: &mut Ctx, _inputs: &CallInputs, outcome: &mut CallOutcome) {
        self.exit(outcome.result.result.is_ok());
    }

    fn create(&mut self, _ctx: &mut Ctx, inputs: &mut CreateInputs) -> Option<CreateOutcome> {
        if self.enabled && !self.checkpoints.is_empty() && !inputs.value().is_zero() {
            self.record_balance(inputs.caller());
        }
        self.enter();
        None
    }

    fn create_end(&mut self, _ctx: &mut Ctx, _inputs: &CreateInputs, outcome: &mut CreateOutcome) {
        self.exit(outcome.result.result.is_ok());
    }
}

#[derive(Debug, Default)]
pub(super) struct ReadSet {
    accounts: AddressMap<AccountRead>,
    storage: StorageSeed,
    block_hashes: BTreeMap<u64, B256>,
    overflowed: bool,
    writes: Vec<StorageWrite>,
    balance_reads: AddressSet,
}

/// Transaction-local writes; a failed frame truncates writes but never its read dependencies.
#[derive(Debug, Default)]
struct StorageJournal {
    writes: Vec<StorageWrite>,
    balance_reads: AddressSet,
    overflowed: bool,
}

#[derive(Debug)]
struct StorageWrite {
    address: Address,
    slot: U256,
    value: U256,
}

#[derive(Debug)]
struct AccountRead {
    info: Option<AccountInfo>,
    fee_only: bool,
}

/// A strict speculative execution together with its original-state dependencies.
#[derive(Debug)]
pub(crate) struct PrewarmResult<H> {
    tx_hash: B256,
    reads: ReadSet,
    result: ExecutionResult<H>,
    effects: StateEffects,
    caller: Address,
    pub(super) refreshed: bool,
}

/// Validated account effects and original storage metadata, with ordered persistent writes.
#[derive(Debug)]
struct StateEffects {
    accounts: EvmState,
    writes: Vec<StorageWrite>,
}

impl<H> PrewarmResult<H> {
    pub(super) fn new(
        tx_hash: B256,
        caller: Address,
        mut reads: ReadSet,
        mut result: ResultAndState<H>,
    ) -> Self {
        // Verify that the recorded operations cover the worker's complete committed storage
        // delta. Fail closed on an unsupported/missing operation rather than replaying a snapshot.
        let mut final_writes = HashMap::<_, _>::default();
        for write in &reads.writes {
            final_writes.insert((write.address, write.slot), write.value);
        }
        for (address, account) in &mut result.state {
            for (slot, value) in &mut account.storage {
                let recorded = final_writes.remove(&(*address, *slot));
                if recorded.is_some_and(|recorded| recorded != value.present_value) ||
                    (value.is_changed() && recorded.is_none())
                {
                    reads.overflowed = true;
                }
                // Retain slot warmth/original-value metadata, not the worker's final value.
                value.present_value = value.original_value;
            }
        }
        reads.overflowed |= !final_writes.is_empty();
        let writes = std::mem::take(&mut reads.writes);
        Self {
            tx_hash,
            reads,
            result: result.result,
            effects: StateEffects { accounts: result.state, writes },
            caller,
            refreshed: false,
        }
    }

    /// Validates all dependencies before changing any canonical state.
    pub(crate) fn validate<DB: Database>(
        mut self,
        tx_hash: B256,
        db: &mut DB,
    ) -> Option<ResultAndState<H>> {
        if self.tx_hash != tx_hash || self.reads.overflowed {
            return None
        }
        for (address, read) in self.reads.accounts {
            let current = db.basic(address).ok()?;
            match (&read.info, &current) {
                (None, None) => {}
                (Some(original), Some(current))
                    if original.nonce == current.nonce &&
                        original.code_hash == current.code_hash =>
                {
                    if current.balance != original.balance {
                        if !read.fee_only && self.reads.balance_reads.contains(&address) {
                            return None
                        }
                        if (read.fee_only || address == self.caller) &&
                            current.balance < original.balance
                        {
                            return None
                        }
                        // Empty-account classification influences call gas and EXTCODEHASH.
                        // Fee-only loads occur after execution and cannot influence those checks.
                        if !read.fee_only && original.is_empty() != current.is_empty() {
                            return None
                        }
                        let account = self.effects.accounts.get_mut(&address)?;
                        account.info.balance = if account.info.balance >= original.balance {
                            current.balance.checked_add(account.info.balance - original.balance)?
                        } else {
                            // Only the caller or a value-debit source may lose balance.
                            // Internal debit sources are exact dependencies recorded by hooks.
                            if address != self.caller {
                                return None
                            }
                            current.balance.checked_sub(original.balance - account.info.balance)?
                        };
                        *account.original_info_mut() = current.clone();
                    }
                }
                _ => return None,
            }
        }
        for ((address, slot), value) in self.reads.storage {
            if db.storage(address, slot).ok()? != value {
                return None
            }
        }
        for (number, hash) in self.reads.block_hashes {
            if db.block_hash(number).ok()? != hash {
                return None
            }
        }
        // All dependencies have passed. Stage the ordered committed writes locally; ordinary
        // executor commit applies account creation/destruction flags and storage clearing.
        for write in self.effects.writes {
            self.effects
                .accounts
                .get_mut(&write.address)?
                .storage
                .get_mut(&write.slot)?
                .present_value = write.value;
        }
        Some(ResultAndState { result: self.result, state: self.effects.accounts })
    }
}

/// Bounded, non-waiting, payload-local handoff between prewarm workers and serial execution.
#[derive(Debug)]
pub(crate) struct PrewarmResults<H> {
    ready: Mutex<ReadyResults<H>>,
    pub(crate) metrics: HandoffMetrics,
    executed_tx: crossbeam_channel::Sender<(usize, Vec<StorageWrite>)>,
    executed_rx: crossbeam_channel::Receiver<(usize, Vec<StorageWrite>)>,
}

impl<H> Default for PrewarmResults<H> {
    fn default() -> Self {
        let (executed_tx, executed_rx) = crossbeam_channel::bounded(MAX_READY_RESULTS * 2);
        Self {
            executed_tx,
            executed_rx,
            ready: Mutex::new(ReadyResults {
                next: 0,
                slots: std::iter::repeat_with(|| None).take(MAX_READY_RESULTS).collect(),
                refresh: RefreshIndex::default(),
                refresh_versions: vec![0; MAX_READY_RESULTS],
            }),
            metrics: HandoffMetrics::default(),
        }
    }
}

impl<H> PrewarmResults<H> {
    pub(super) const fn within_window(index: usize, next: usize) -> bool {
        index >= next && index.saturating_sub(next) < MAX_READY_RESULTS
    }

    pub(super) fn publish(&self, index: usize, next: usize, result: PrewarmResult<H>) {
        if !Self::within_window(index, next) || result.reads.overflowed {
            return
        }
        let mut ready = self.ready.lock();
        if Self::within_window(index, next.max(ready.next)) {
            if result.refreshed {
                self.metrics.refreshed.increment(1);
            }
            ready.refresh.replace(index, &result.effects.writes);
            ready.slots[index % MAX_READY_RESULTS] = Some((index, result));
        }
    }

    pub(crate) fn take(&self, index: usize) -> Option<PrewarmResult<H>> {
        let mut ready = self.ready.lock();
        if index < ready.next {
            return None
        }
        ready.next = index.saturating_add(1);
        let slot = &mut ready.slots[index % MAX_READY_RESULTS];
        if slot.as_ref().is_some_and(|(ready_index, _)| *ready_index == index) {
            slot.take().map(|(_, result)| result)
        } else {
            None
        }
    }

    /// Collects actual writes without waiting for the refresh worker. Missing reports only reduce
    /// prediction quality: every retry still passes canonical dependency validation.
    pub(crate) fn report_execution(&self, index: usize, state: &EvmState) {
        let mut writes = Vec::new();
        for (address, account) in state {
            if !account.is_touched() {
                continue;
            }
            for (slot, value) in &account.storage {
                if value.is_changed() {
                    if writes.len() == MAX_READS {
                        return;
                    }
                    writes.push(StorageWrite {
                        address: *address,
                        slot: *slot,
                        value: value.present_value,
                    });
                }
            }
        }
        if self.executed_tx.try_send((index, writes)).is_err() {
            self.metrics.refresh_reports_dropped.increment(1);
        }
    }

    /// Finds the nearest ready result whose storage inputs conflict with earlier writes. Each
    /// result gets at most one attempt per prediction generation, within a block-wide budget.
    pub(super) fn take_refresh(
        &self,
        next: usize,
        has_source: impl Fn(usize) -> bool,
    ) -> Option<(usize, StorageSeed)> {
        let mut ready = self.ready.lock();
        for (index, writes) in self.executed_rx.try_iter() {
            ready.refresh.replace(index, &writes);
        }
        if ready.refresh.disabled || ready.refresh.attempts >= MAX_REFRESHES {
            return None;
        }
        let next = next.max(ready.next);
        for index in next.saturating_add(1)..next.saturating_add(MAX_READY_RESULTS) {
            let offset = index % MAX_READY_RESULTS;
            if !has_source(index) || ready.refresh_versions[offset] == ready.refresh.version {
                continue;
            }
            let Some((stored_index, result)) = &ready.slots[offset] else {
                continue;
            };
            if *stored_index != index {
                continue;
            }
            let mut changed = false;
            let mut seed = HashMap::default();
            for (key, original) in &result.reads.storage {
                if let Some(value) = ready.refresh.value_before(key, index) {
                    changed |= value != *original;
                    seed.insert(*key, value);
                }
            }
            ready.refresh_versions[offset] = ready.refresh.version;
            if changed {
                ready.refresh.attempts += 1;
                self.metrics.storage_conflicts.increment(1);
                return Some((index, seed));
            }
        }
        None
    }
}

/// Private storage inputs supplied to one speculative retry.
type StorageSeed = HashMap<(Address, U256), U256>;

/// Bounded predictions used only to seed speculative retries, never canonical execution.
#[derive(Debug, Default)]
struct RefreshIndex {
    writes: HashMap<(Address, U256), BTreeMap<usize, U256>>,
    by_transaction: BTreeMap<usize, Vec<(Address, U256)>>,
    entries: usize,
    version: u64,
    attempts: usize,
    disabled: bool,
}

impl RefreshIndex {
    fn replace(&mut self, index: usize, writes: &[StorageWrite]) {
        if self.disabled {
            return;
        }
        if let Some(keys) = self.by_transaction.remove(&index) {
            for key in keys {
                if let Some(writers) = self.writes.get_mut(&key) {
                    self.entries -= usize::from(writers.remove(&index).is_some());
                    if writers.is_empty() {
                        self.writes.remove(&key);
                    }
                }
            }
        }
        for write in writes {
            let writers = self.writes.entry((write.address, write.slot)).or_default();
            if writers.insert(index, write.value).is_none() {
                self.entries += 1;
                self.by_transaction.entry(index).or_default().push((write.address, write.slot));
            }
            if self.entries > MAX_READS || writers.len() > 64 {
                self.writes.clear();
                self.by_transaction.clear();
                self.disabled = true;
                return;
            }
        }
        self.version = self.version.wrapping_add(1);
    }

    fn value_before(&self, key: &(Address, U256), index: usize) -> Option<U256> {
        self.writes.get(key)?.range(..index).next_back().map(|(_, value)| *value)
    }
}

#[derive(Debug)]
struct ReadyResults<H> {
    next: usize,
    slots: Vec<Option<(usize, PrewarmResult<H>)>>,
    refresh: RefreshIndex,
    refresh_versions: Vec<u64>,
}

/// Counters for validated handoffs and ordinary-execution fallbacks.
#[derive(Clone, Metrics)]
#[metrics(scope = "sync.prewarm.handoff")]
pub(crate) struct HandoffMetrics {
    /// Transactions committed from validated speculative execution.
    pub(crate) reused: Counter,
    /// Ready results rejected by dependency or block-gas validation.
    pub(crate) rejected: Counter,
    /// Transactions whose speculative result was not ready.
    pub(crate) missing: Counter,
    /// Ready results found to have conflicting predicted storage inputs.
    pub(crate) storage_conflicts: Counter,
    /// Speculative executions retried with updated storage inputs.
    pub(crate) refresh_attempts: Counter,
    /// Refreshed results published before canonical execution reached them.
    pub(crate) refreshed: Counter,
    /// Refreshed results accepted by canonical dependency validation.
    pub(crate) refresh_reused: Counter,
    /// Actual-write reports dropped because the refresh queue was full.
    pub(crate) refresh_reports_dropped: Counter,
}

/// Attempts a non-waiting handoff through the canonical executor's normal commit path.
pub(crate) fn try_reuse_transaction<Cfg, E>(
    results: &PrewarmResults<HaltReasonFor<Cfg>>,
    index: usize,
    transaction: &TxTy<Cfg::Primitives>,
    config: &Cfg,
    executor: &mut E,
) -> bool
where
    Cfg: ConfigureEvm,
    E: BlockExecutor<
        Transaction = TxTy<Cfg::Primitives>,
        Result = TxExecutionResultFor<Cfg>,
        Receipt: TxReceipt,
        Evm: Evm<HaltReason = HaltReasonFor<Cfg>, Spec = SpecFor<Cfg>>,
    >,
{
    let Some(candidate) = results.take(index) else {
        results.metrics.missing.increment(1);
        return false
    };
    let refreshed = candidate.refreshed;
    let gas_used = executor.receipts().last().map_or(0, TxReceipt::cumulative_gas_used);
    let evm = executor.evm_mut();
    let available = evm.block().gas_limit().saturating_sub(gas_used);
    let limit = transaction.gas_limit().min(evm.cfg_env().tx_gas_limit_cap());
    if limit <= available &&
        let Some(result) = candidate.validate(*transaction.tx_hash(), evm.db_mut()) &&
        let Some(result) = config.prewarm_transaction_result(transaction, result)
    {
        results.report_execution(index, &result.result().state);
        executor.commit_transaction(result);
        if refreshed {
            results.metrics.refresh_reused.increment(1);
        }
        results.metrics.reused.increment(1);
        true
    } else {
        results.metrics.rejected.increment(1);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{transaction::Recovered, Header, Signed, TxLegacy};
    use alloy_primitives::{Bytes, Signature, TxKind};
    use reth_ethereum_primitives::{Block as EthBlock, BlockBody, Receipt, TransactionSigned};
    use reth_evm::{ConfigureEvm, Evm, EvmEnvFor, RecoveredTx};
    use reth_evm_ethereum::EthEvmConfig;
    use reth_primitives_traits::Block as _;
    use revm::{
        context::{result::HaltReason, TxEnv},
        database::{states::bundle_state::BundleRetention, BundleState, InMemoryDB, State},
        primitives::hardfork::SpecId,
        DatabaseCommit,
    };

    const BENEFICIARY: Address = Address::repeat_byte(3);
    const CONTRACT: Address = Address::repeat_byte(4);

    fn env() -> EvmEnvFor<EthEvmConfig> {
        let mut env = EvmEnvFor::<EthEvmConfig>::default();
        env.cfg_env.set_spec_and_mainnet_gas_params(SpecId::CANCUN);
        env.block_env.gas_limit = 30_000_000;
        env.block_env.beneficiary = BENEFICIARY;
        env
    }

    fn database() -> InMemoryDB {
        let mut db = InMemoryDB::default();
        for address in [Address::repeat_byte(1), Address::repeat_byte(2), BENEFICIARY, CONTRACT] {
            db.insert_account_info(
                address,
                AccountInfo { balance: U256::from(1_000_000_000), ..Default::default() },
            );
        }
        db
    }

    fn tx(sender: u8) -> TxEnv {
        TxEnv {
            caller: Address::repeat_byte(sender),
            gas_limit: 100_000,
            gas_price: 1,
            kind: TxKind::Call(CONTRACT),
            value: U256::ZERO,
            ..Default::default()
        }
    }

    fn speculate(db: InMemoryDB, tx: TxEnv) -> PrewarmResult<HaltReason> {
        let caller = tx.caller;
        let recording = RecordingDatabase::new(db, BENEFICIARY, true);
        let inspector = recording.inspector();
        let mut evm =
            EthEvmConfig::mainnet().evm_with_env_and_inspector(recording, env(), inspector);
        evm.enable_inspector();
        let result = evm.transact(tx).unwrap();
        let reads = evm.db_mut().take_reads();
        PrewarmResult::new(B256::ZERO, caller, reads, result)
    }

    fn execute(db: &mut InMemoryDB, tx: TxEnv) -> ResultAndState<HaltReason> {
        EthEvmConfig::mainnet().evm_with_env(db, env()).transact(tx).unwrap()
    }

    fn contract(db: &mut InMemoryDB, code: &[u8]) {
        db.insert_account_info(
            CONTRACT,
            AccountInfo::from_bytecode(Bytecode::new_raw(Bytes::copy_from_slice(code))),
        );
    }

    #[test]
    fn unchanged_state_reuses_identical_execution_result() {
        let mut canonical = database();
        let candidate = speculate(canonical.clone(), tx(1));
        let reused = candidate.validate(B256::ZERO, &mut canonical).unwrap();
        let serial = execute(&mut canonical, tx(1));
        assert_eq!(reused, serial);
    }

    #[test]
    fn independent_transactions_rebase_beneficiary_reward() {
        let mut canonical = database();
        let candidate = speculate(canonical.clone(), tx(2));
        assert!(candidate.reads.accounts[&BENEFICIARY].fee_only);
        let first = execute(&mut canonical, tx(1));
        canonical.commit(first.state);
        let reused = candidate.validate(B256::ZERO, &mut canonical).unwrap();
        let serial = execute(&mut canonical, tx(2));
        assert_eq!(reused, serial);
    }

    #[test]
    fn beneficiary_balance_read_by_evm_is_not_rebased() {
        let mut canonical = database();
        contract(&mut canonical, &[0x41, 0x31, 0x60, 0x00, 0x55, 0x00]);
        let candidate = speculate(canonical.clone(), tx(2));
        assert!(!candidate.reads.accounts[&BENEFICIARY].fee_only);
        let first = execute(&mut canonical, tx(1));
        canonical.commit(first.state);
        assert!(candidate.validate(B256::ZERO, &mut canonical).is_none());
    }

    #[test]
    fn beneficiary_as_recipient_replays_balance_addition() {
        let mut canonical = database();
        let mut transaction = tx(2);
        transaction.kind = TxKind::Call(BENEFICIARY);
        transaction.value = U256::from(7);
        let candidate = speculate(canonical.clone(), transaction.clone());
        assert!(!candidate.reads.accounts[&BENEFICIARY].fee_only);
        let first = execute(&mut canonical, tx(1));
        canonical.commit(first.state);
        let reused = candidate.validate(B256::ZERO, &mut canonical).unwrap();
        assert_eq!(reused, execute(&mut canonical, transaction));
    }

    #[test]
    fn earlier_sender_transaction_rejects_stale_nonce() {
        let mut canonical = database();
        let candidate = speculate(canonical.clone(), tx(1));
        let first = execute(&mut canonical, tx(1));
        canonical.commit(first.state);
        assert!(candidate.validate(B256::ZERO, &mut canonical).is_none());
    }

    #[test]
    fn changed_sender_balance_rejects_result() {
        let mut canonical = database();
        let candidate = speculate(canonical.clone(), tx(1));
        canonical.insert_account_info(
            Address::repeat_byte(1),
            AccountInfo { balance: U256::from(200_000_000), ..Default::default() },
        );
        assert!(candidate.validate(B256::ZERO, &mut canonical).is_none());
    }

    #[test]
    fn increased_sender_balance_replays_debit() {
        let mut canonical = database();
        let candidate = speculate(canonical.clone(), tx(1));
        canonical.insert_account_info(
            Address::repeat_byte(1),
            AccountInfo { balance: U256::from(2_000_000_000), ..Default::default() },
        );
        let reused = candidate.validate(B256::ZERO, &mut canonical).unwrap();
        assert_eq!(reused, execute(&mut canonical, tx(1)));
    }

    #[test]
    fn committed_storage_actions_replay_in_order() {
        let mut canonical = database();
        contract(&mut canonical, &[0x60, 1, 0x60, 0, 0x55, 0x60, 2, 0x60, 0, 0x55, 0]);
        let candidate = speculate(canonical.clone(), tx(1));
        assert_eq!(
            candidate.effects.writes.iter().map(|write| write.value).collect::<Vec<_>>(),
            vec![U256::from(1), U256::from(2)]
        );
        assert_eq!(
            candidate.effects.accounts[&CONTRACT].storage[&U256::ZERO].present_value,
            U256::ZERO
        );
        let reused = candidate.validate(B256::ZERO, &mut canonical).unwrap();
        assert_eq!(reused, execute(&mut canonical, tx(1)));
    }

    #[test]
    fn reverted_storage_actions_are_discarded_but_reads_remain() {
        let mut canonical = database();
        contract(&mut canonical, &[0x60, 1, 0x60, 0, 0x55, 0x60, 0, 0x60, 0, 0xfd]);
        let candidate = speculate(canonical.clone(), tx(1));
        assert!(candidate.effects.writes.is_empty());
        assert_eq!(candidate.reads.storage[&(CONTRACT, U256::ZERO)], U256::ZERO);
        let reused = candidate.validate(B256::ZERO, &mut canonical).unwrap();
        assert_eq!(reused, execute(&mut canonical, tx(1)));
        let candidate = speculate(canonical.clone(), tx(1));
        canonical.insert_account_storage(CONTRACT, U256::ZERO, U256::from(2)).unwrap();
        assert!(candidate.validate(B256::ZERO, &mut canonical).is_none());
    }

    #[test]
    fn nested_revert_discards_only_child_actions() {
        let mut canonical = database();
        let child = Address::repeat_byte(5);
        canonical.insert_account_info(
            child,
            AccountInfo::from_bytecode(Bytecode::new_raw(Bytes::from_static(&[
                0x60, 2, 0x60, 0, 0x55, 0x60, 0, 0x60, 0, 0xfd,
            ]))),
        );
        // Parent writes, calls reverting child, then writes again.
        let mut code = vec![0x60, 1, 0x60, 0, 0x55];
        code.extend_from_slice(&[0x60, 0, 0x60, 0, 0x60, 0, 0x60, 0, 0x60, 0, 0x73]);
        code.extend_from_slice(child.as_slice());
        code.extend_from_slice(&[0x61, 0x80, 0x00, 0xf1, 0x50, 0x60, 3, 0x60, 1, 0x55, 0]);
        contract(&mut canonical, &code);
        let candidate = speculate(canonical.clone(), tx(1));
        assert_eq!(candidate.effects.writes.len(), 2);
        assert!(candidate.effects.writes.iter().all(|write| write.address == CONTRACT));
        assert_eq!(candidate.reads.storage[&(child, U256::ZERO)], U256::ZERO);
        let reused = candidate.validate(B256::ZERO, &mut canonical).unwrap();
        assert_eq!(reused, execute(&mut canonical, tx(1)));
    }

    #[test]
    fn missing_storage_action_fails_closed() {
        let mut canonical = database();
        contract(&mut canonical, &[0x60, 1, 0x60, 0, 0x55, 0]);
        let result = execute(&mut canonical, tx(1));
        let candidate = PrewarmResult::new(B256::ZERO, tx(1).caller, ReadSet::default(), result);
        assert!(candidate.validate(B256::ZERO, &mut canonical).is_none());
    }

    #[test]
    fn selfbalance_is_an_exact_dependency() {
        let mut canonical = database();
        contract(&mut canonical, &[0x47, 0x60, 0, 0x55, 0]);
        let candidate = speculate(canonical.clone(), tx(1));
        let mut info = canonical.basic(CONTRACT).unwrap().unwrap();
        info.balance = U256::from(7);
        canonical.insert_account_info(CONTRACT, info);
        assert!(candidate.validate(B256::ZERO, &mut canonical).is_none());
    }

    #[test]
    fn unobserved_recipient_balance_replays_credit() {
        let mut canonical = database();
        contract(&mut canonical, &[0]);
        let mut transaction = tx(1);
        transaction.value = U256::from(7);
        let candidate = speculate(canonical.clone(), transaction.clone());
        let mut info = canonical.basic(CONTRACT).unwrap().unwrap();
        info.balance = U256::from(9);
        canonical.insert_account_info(CONTRACT, info);
        let reused = candidate.validate(B256::ZERO, &mut canonical).unwrap();
        assert_eq!(reused.state[&CONTRACT].info.balance, U256::from(16));
        assert_eq!(reused, execute(&mut canonical, transaction));
    }

    #[test]
    fn reverted_balance_read_remains_an_exact_dependency() {
        let mut canonical = database();
        contract(&mut canonical, &[0x41, 0x31, 0x50, 0x60, 0, 0x60, 0, 0xfd]);
        let candidate = speculate(canonical.clone(), tx(1));
        assert!(candidate.effects.writes.is_empty());
        assert!(candidate.reads.balance_reads.contains(&BENEFICIARY));
        canonical.insert_account_info(
            BENEFICIARY,
            AccountInfo { balance: U256::from(2_000_000_000), ..Default::default() },
        );
        assert!(candidate.validate(B256::ZERO, &mut canonical).is_none());
    }

    #[test]
    fn created_contract_storage_replays_with_creation_metadata() {
        let mut canonical = database();
        let mut transaction = tx(1);
        transaction.kind = TxKind::Create;
        transaction.data = Bytes::from_static(&[0x60, 1, 0x60, 0, 0x55, 0x60, 0, 0x60, 0, 0xf3]);
        let candidate = speculate(canonical.clone(), transaction.clone());
        assert_eq!(candidate.effects.writes.len(), 1);
        let reused = candidate.validate(B256::ZERO, &mut canonical).unwrap();
        assert_eq!(reused, execute(&mut canonical, transaction));
    }

    #[test]
    fn failed_internal_value_check_is_an_exact_dependency() {
        let mut canonical = database();
        let mut code = vec![0x60, 0, 0x60, 0, 0x60, 0, 0x60, 0, 0x60, 2, 0x73];
        code.extend_from_slice(Address::repeat_byte(5).as_slice());
        code.extend_from_slice(&[0x61, 0x80, 0x00, 0xf1, 0x50, 0]);
        contract(&mut canonical, &code);
        let mut info = canonical.basic(CONTRACT).unwrap().unwrap();
        info.balance = U256::from(1);
        canonical.insert_account_info(CONTRACT, info.clone());
        let candidate = speculate(canonical.clone(), tx(1));
        assert!(candidate.reads.balance_reads.contains(&CONTRACT));
        info.balance = U256::from(3);
        canonical.insert_account_info(CONTRACT, info);
        assert!(candidate.validate(B256::ZERO, &mut canonical).is_none());
    }

    #[test]
    fn balance_rebase_rejects_changed_empty_classification() {
        let mut canonical = database();
        contract(&mut canonical, &[0]);
        let mut transaction = tx(1);
        transaction.kind = TxKind::Call(Address::repeat_byte(5));
        canonical.insert_account_info(Address::repeat_byte(5), AccountInfo::default());
        let candidate = speculate(canonical.clone(), transaction);
        canonical.insert_account_info(
            Address::repeat_byte(5),
            AccountInfo { balance: U256::from(1), ..Default::default() },
        );
        assert!(candidate.validate(B256::ZERO, &mut canonical).is_none());
    }

    #[test]
    fn write_action_limit_rejects_even_reverted_frames() {
        let mut canonical = database();
        // Repeated writes to one slot exceed the action limit without exceeding the read limit.
        // The eventual out-of-gas revert must not clear the overflow marker.
        contract(&mut canonical, &[0x5b, 0x60, 1, 0x60, 0, 0x55, 0x60, 0, 0x56]);
        let mut transaction = tx(1);
        transaction.gas_limit = 3_000_000;
        let candidate = speculate(canonical.clone(), transaction);
        assert!(candidate.reads.overflowed);
        assert!(candidate.effects.writes.is_empty());
        assert!(candidate.validate(B256::ZERO, &mut canonical).is_none());
    }

    #[test]
    fn uninspected_bytecode_frame_fails_closed() {
        let mut recording = RecordingDatabase::new(database(), BENEFICIARY, true);
        let mut inspector = recording.inspector();
        inspector.enter();
        inspector.checkpoints.last_mut().unwrap().has_code = true;
        inspector.exit(true);
        assert!(recording.take_reads().overflowed);
    }

    #[test]
    fn uninspected_resumed_segment_fails_closed() {
        let mut recording = RecordingDatabase::new(database(), BENEFICIARY, true);
        let mut inspector = recording.inspector();
        inspector.enter();
        let frame = inspector.checkpoints.last_mut().unwrap();
        frame.has_code = true;
        frame.stepped = true;
        inspector.enter(); // Parent yielded a call after an inspected segment.
        inspector.exit(true); // An empty/precompile child has no opcodes to inspect.
        inspector.exit(true); // Resumed parent completed without opcode callbacks.
        assert!(recording.take_reads().overflowed);
    }

    #[test]
    fn empty_frame_needs_no_opcode_callbacks() {
        let mut recording = RecordingDatabase::new(database(), BENEFICIARY, true);
        let mut inspector = recording.inspector();
        inspector.enter();
        inspector.exit(true);
        assert!(!recording.take_reads().overflowed);
    }

    proptest::proptest! {
        #[test]
        fn storage_action_replay_matches_serial(
            writes in proptest::collection::vec((0u8..8, 0u8..8), 0..40),
            reverted in proptest::bool::ANY,
            original in 0u8..8,
        ) {
            let mut canonical = database();
            let mut code = Vec::new();
            for (slot, value) in writes {
                code.extend_from_slice(&[0x60, value, 0x60, slot, 0x55]);
            }
            if reverted {
                code.extend_from_slice(&[0x60, 0, 0x60, 0, 0xfd]);
            } else {
                code.push(0);
            }
            contract(&mut canonical, &code);
            for slot in 0..8 {
                canonical.insert_account_storage(CONTRACT, U256::from(slot), U256::from(original)).unwrap();
            }
            let mut transaction = tx(1);
            transaction.gas_limit = 1_000_000;
            let candidate = speculate(canonical.clone(), transaction.clone());
            let reused = candidate.validate(B256::ZERO, &mut canonical).unwrap();
            proptest::prop_assert_eq!(reused, execute(&mut canonical, transaction));
        }
    }

    #[test]
    fn storage_write_checks_original_slot_value() {
        let mut canonical = database();
        contract(&mut canonical, &[0x60, 0x01, 0x60, 0x00, 0x55, 0x00]);
        let candidate = speculate(canonical.clone(), tx(1));
        assert_eq!(candidate.reads.storage[&(CONTRACT, U256::ZERO)], U256::ZERO);
        canonical.insert_account_storage(CONTRACT, U256::ZERO, U256::from(2)).unwrap();
        assert!(candidate.validate(B256::ZERO, &mut canonical).is_none());
    }

    #[test]
    fn changed_code_or_account_existence_rejects_result() {
        for remove in [false, true] {
            let mut canonical = database();
            let candidate = speculate(canonical.clone(), tx(1));
            if remove {
                canonical.cache.accounts.remove(&CONTRACT);
            } else {
                contract(&mut canonical, &[0x00]);
            }
            assert!(candidate.validate(B256::ZERO, &mut canonical).is_none());
        }
    }

    #[test]
    fn revert_halt_create_and_selfdestruct_match_serial_execution() {
        for code in [
            &[0x60, 0x00, 0x60, 0x00, 0xfd][..],
            &[0xfe][..],
            &[0x60, 0x05, 0xff][..],
            &[0x60, 0x00, 0x60, 0x00, 0x60, 0x00, 0xf0, 0x00][..],
        ] {
            let mut canonical = database();
            contract(&mut canonical, code);
            let candidate = speculate(canonical.clone(), tx(2));
            let first = execute(
                &mut canonical,
                TxEnv { kind: TxKind::Call(Address::repeat_byte(9)), ..tx(1) },
            );
            canonical.commit(first.state);
            let reused = candidate.validate(B256::ZERO, &mut canonical).unwrap();
            assert_eq!(reused, execute(&mut canonical, tx(2)));
        }
        let mut canonical = database();
        let transaction =
            TxEnv { kind: TxKind::Create, data: Bytes::from_static(&[0x00]), ..tx(1) };
        let candidate = speculate(canonical.clone(), transaction.clone());
        let reused = candidate.validate(B256::ZERO, &mut canonical).unwrap();
        assert_eq!(reused, execute(&mut canonical, transaction));
    }

    #[test]
    fn wrong_transaction_hash_rejects_result() {
        let mut canonical = database();
        let candidate = speculate(canonical.clone(), tx(1));
        assert!(candidate.validate(B256::repeat_byte(1), &mut canonical).is_none());
    }

    #[test]
    fn oversized_read_set_cannot_be_reused() {
        let mut recording = RecordingDatabase::new(database(), BENEFICIARY, true);
        for slot in 0..=MAX_READS {
            recording.storage(CONTRACT, U256::from(slot)).unwrap();
        }
        let reads = recording.take_reads();
        assert!(reads.overflowed);
        assert!(reads.storage.is_empty());
        let mut candidate = speculate(database(), tx(1));
        candidate.reads = reads;
        assert!(candidate.validate(B256::ZERO, &mut database()).is_none());
    }

    #[test]
    fn ready_results_are_bounded_and_consumed_once() {
        let results = PrewarmResults::default();
        for index in 0..=MAX_READY_RESULTS {
            results.publish(index, 0, speculate(database(), tx(1)));
        }
        assert_eq!(results.ready.lock().slots.iter().flatten().count(), MAX_READY_RESULTS);
        assert!(results.take(1).is_some());
        assert!(results.take(1).is_none());
        assert!(results.take(0).is_none());
        assert!(results.take(MAX_READY_RESULTS).is_none());
        results.publish(0, 1, speculate(database(), tx(1)));
        assert!(results.take(0).is_none());
    }

    #[test]
    fn ready_results_wrap_without_consuming_stale_slots() {
        let results = PrewarmResults::default();
        results.publish(0, 0, speculate(database(), tx(1)));
        assert!(results.take(MAX_READY_RESULTS).is_none());
        results.publish(0, 0, speculate(database(), tx(1)));
        assert!(results.take(0).is_none());
        results.publish(MAX_READY_RESULTS + 1, MAX_READY_RESULTS + 1, speculate(database(), tx(1)));
        assert!(results.take(MAX_READY_RESULTS + 1).is_some());
        assert!(results.take(MAX_READY_RESULTS + 1).is_none());
        // Publishing with the main loop's next index must not invalidate a ready current result.
        results.publish(MAX_READY_RESULTS + 2, MAX_READY_RESULTS + 2, speculate(database(), tx(1)));
        results.publish(MAX_READY_RESULTS + 3, MAX_READY_RESULTS + 3, speculate(database(), tx(1)));
        assert!(results.take(MAX_READY_RESULTS + 2).is_some());
    }

    #[test]
    fn borrowed_proof_hints_preserve_execution_state() {
        let mut parent = database();
        contract(&mut parent, &[0x60, 0x01, 0x60, 0x00, 0x55, 0x00]);
        let state = execute(&mut parent, tx(1)).state;
        let original_state = state.clone();
        let (targets, count) = reth_trie_common::MultiProofTargetsV2::from_state_ref(&state);
        assert_eq!(count, 1);
        assert_eq!(
            targets.storage_targets[&alloy_primitives::keccak256(CONTRACT)][0].key(),
            alloy_primitives::keccak256(B256::ZERO)
        );
        assert_eq!(state, original_state);
        let (owned, owned_count) =
            reth_trie_common::MultiProofTargetsV2::from_state(original_state);
        assert_eq!(count, owned_count);
        assert_eq!(
            targets.account_targets.iter().map(|target| target.key()).collect::<Vec<_>>(),
            owned.account_targets.iter().map(|target| target.key()).collect::<Vec<_>>()
        );
    }

    fn recovered(transaction: &TxEnv) -> Recovered<TransactionSigned> {
        Recovered::new_unchecked(
            TransactionSigned::Legacy(Signed::new_unchecked(
                TxLegacy {
                    nonce: transaction.nonce,
                    gas_price: transaction.gas_price,
                    gas_limit: transaction.gas_limit,
                    to: transaction.kind,
                    value: transaction.value,
                    input: transaction.data.clone(),
                    ..Default::default()
                },
                Signature::test_signature(),
                B256::ZERO,
            )),
            transaction.caller,
        )
    }

    fn execute_block_transactions(
        parent: InMemoryDB,
        transactions: &[TxEnv],
        handoff: bool,
        gas_limit: u64,
    ) -> (Vec<Receipt>, BundleState, usize) {
        let config = EthEvmConfig::mainnet();
        let block = EthBlock {
            header: Header { gas_limit, ..Default::default() },
            body: BlockBody::default(),
        }
        .seal_slow();
        let mut state = State::builder().with_database(parent.clone()).with_bundle_update().build();
        let mut block_env = env();
        block_env.block_env.gas_limit = gas_limit;
        let evm = config.evm_with_env(&mut state, block_env);
        let mut executor =
            config.create_executor_with_state(evm, config.context_for_block(&block).unwrap());
        let results = PrewarmResults::default();
        let mut reused = 0;
        for (index, transaction) in transactions.iter().enumerate() {
            let signed = recovered(transaction);
            if handoff {
                results.publish(index, index, speculate(parent.clone(), transaction.clone()));
            }
            if handoff &&
                try_reuse_transaction(&results, index, signed.tx(), &config, &mut executor)
            {
                reused += 1;
            } else {
                executor.execute_transaction((transaction.clone(), &signed)).unwrap();
            }
        }
        let receipts = executor.receipts().to_vec();
        drop(executor);
        state.merge_transitions(BundleRetention::Reverts);
        (receipts, state.take_bundle(), reused)
    }

    #[test]
    fn canonical_commit_matches_serial_receipts_and_bundle() {
        let transactions = [tx(1), tx(2)];
        let (serial_receipts, serial_state, _) =
            execute_block_transactions(database(), &transactions, false, 30_000_000);
        let (reused_receipts, reused_state, reused) =
            execute_block_transactions(database(), &transactions, true, 30_000_000);
        assert_eq!(reused, 2);
        assert_eq!(reused_receipts, serial_receipts);
        assert_eq!(reused_state, serial_state);
    }

    #[test]
    fn canonical_storage_conflict_falls_back_and_matches_serial() {
        let mut parent = database();
        contract(&mut parent, &[0x60, 0x01, 0x60, 0x00, 0x55, 0x00]);
        let transactions = [tx(1), tx(2)];
        let (serial_receipts, serial_state, _) =
            execute_block_transactions(parent.clone(), &transactions, false, 30_000_000);
        let (reused_receipts, reused_state, reused) =
            execute_block_transactions(parent, &transactions, true, 30_000_000);
        assert_eq!(reused, 1);
        assert_eq!(reused_receipts, serial_receipts);
        assert_eq!(reused_state, serial_state);
    }

    #[test]
    fn ready_result_cannot_bypass_remaining_block_gas() {
        let config = EthEvmConfig::mainnet();
        let parent = database();
        let block = EthBlock {
            header: Header { gas_limit: 110_000, ..Default::default() },
            body: BlockBody::default(),
        }
        .seal_slow();
        let mut state = State::builder().with_database(parent.clone()).with_bundle_update().build();
        let mut block_env = env();
        block_env.block_env.gas_limit = 110_000;
        let evm = config.evm_with_env(&mut state, block_env);
        let mut executor =
            config.create_executor_with_state(evm, config.context_for_block(&block).unwrap());
        executor.execute_transaction((tx(1), recovered(&tx(1)))).unwrap();
        let results = PrewarmResults::default();
        results.publish(1, 1, speculate(parent, tx(2)));
        let signed = recovered(&tx(2));
        assert!(!try_reuse_transaction(&results, 1, signed.tx(), &config, &mut executor));
        assert_eq!(executor.receipts().len(), 1);
        assert!(matches!(executor.execute_transaction((tx(2), signed)), Err(alloy_evm::block::BlockExecutionError::Validation(
            alloy_evm::block::BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas { .. }
        ))));
    }

    #[test]
    fn handoff_is_opt_in_and_rejects_relaxed_or_multidimensional_execution() {
        let config = EthEvmConfig::mainnet();
        assert!(!crate::tree::TreeConfig::default().prewarm_handoff_enabled());
        assert!(crate::tree::TreeConfig::default()
            .with_prewarm_handoff(true)
            .prewarm_handoff_enabled());
        assert!(config.prewarm_handoff_enabled(&env()));
        for flag in 0..3 {
            let mut invalid = env();
            match flag {
                0 => invalid.cfg_env.disable_nonce_check = true,
                1 => invalid.cfg_env.disable_balance_check = true,
                _ => invalid.cfg_env.enable_amsterdam_eip8037 = true,
            }
            assert!(!config.prewarm_handoff_enabled(&invalid));
        }
    }
    fn speculate_seeded(
        db: InMemoryDB,
        transaction: TxEnv,
        seed: StorageSeed,
    ) -> PrewarmResult<HaltReason> {
        let caller = transaction.caller;
        let mut recording = RecordingDatabase::new(db, BENEFICIARY, true);
        recording.seed_storage(seed);
        let inspector = recording.inspector();
        let mut evm =
            EthEvmConfig::mainnet().evm_with_env_and_inspector(recording, env(), inspector);
        evm.enable_inspector();
        let result = evm.transact(transaction).unwrap();
        let reads = evm.db_mut().take_reads();
        let mut candidate = PrewarmResult::new(B256::ZERO, caller, reads, result);
        candidate.refreshed = true;
        candidate
    }

    #[test]
    fn refresh_counter_chain_matches_serial_receipts_and_bundle() {
        let mut parent = database();
        // Increment slot zero; each later transaction depends on the previous increment.
        contract(&mut parent, &[0x60, 0, 0x54, 0x60, 1, 0x01, 0x60, 0, 0x55, 0]);
        let mut third = tx(2);
        third.caller = BENEFICIARY;
        let transactions = [tx(1), tx(2), third];
        let (serial_receipts, serial_state, _) =
            execute_block_transactions(parent.clone(), &transactions, false, 30_000_000);
        let config = EthEvmConfig::mainnet();
        let block = EthBlock {
            header: Header { gas_limit: 30_000_000, ..Default::default() },
            body: BlockBody::default(),
        }
        .seal_slow();
        let mut state = State::builder().with_database(parent.clone()).with_bundle_update().build();
        let evm = config.evm_with_env(&mut state, env());
        let mut executor =
            config.create_executor_with_state(evm, config.context_for_block(&block).unwrap());
        let results = PrewarmResults::default();
        for (index, transaction) in transactions.iter().enumerate() {
            results.publish(index, 0, speculate(parent.clone(), transaction.clone()));
        }
        while let Some((index, seed)) = results.take_refresh(0, |_| true) {
            results.publish(
                index,
                0,
                speculate_seeded(parent.clone(), transactions[index].clone(), seed),
            );
        }
        for (index, transaction) in transactions.iter().enumerate() {
            let signed = recovered(transaction);
            assert!(try_reuse_transaction(&results, index, signed.tx(), &config, &mut executor));
        }
        let receipts = executor.receipts().to_vec();
        drop(executor);
        state.merge_transitions(BundleRetention::Reverts);
        assert_eq!(receipts, serial_receipts);
        assert_eq!(state.take_bundle(), serial_state);
    }

    #[test]
    fn refreshed_prediction_must_match_actual_storage() {
        let mut parent = database();
        contract(&mut parent, &[0x60, 0, 0x54, 0x60, 1, 0x01, 0x60, 0, 0x55, 0]);
        let seed = HashMap::from_iter([((CONTRACT, U256::ZERO), U256::from(7))]);
        let candidate = speculate_seeded(parent.clone(), tx(2), seed);
        assert!(candidate.validate(B256::ZERO, &mut parent).is_none());
    }

    #[test]
    fn executed_writes_replace_speculative_prediction() {
        let mut parent = database();
        contract(&mut parent, &[0x60, 0, 0x54, 0x60, 1, 0x01, 0x60, 0, 0x55, 0]);
        let results = PrewarmResults::default();
        results.publish(0, 0, speculate(parent.clone(), tx(1)));
        results.publish(2, 0, speculate(parent.clone(), tx(2)));
        let mut actual = execute(&mut parent, tx(1)).state;
        actual.get_mut(&CONTRACT).unwrap().storage.get_mut(&U256::ZERO).unwrap().present_value =
            U256::from(9);
        results.report_execution(0, &actual);
        let (index, seed) = results.take_refresh(0, |_| true).unwrap();
        assert_eq!(index, 2);
        assert_eq!(seed[&(CONTRACT, U256::ZERO)], U256::from(9));
        // An empty actual delta retracts the speculative write entirely.
        results.report_execution(0, &EvmState::default());
        assert!(results.take_refresh(0, |_| true).is_none());
        assert!(results.ready.lock().refresh.value_before(&(CONTRACT, U256::ZERO), 2).is_none());
    }

    #[test]
    fn refresh_budget_and_prediction_memory_are_bounded() {
        let mut parent = database();
        contract(&mut parent, &[0x60, 0, 0x54, 0x60, 1, 0x01, 0x60, 0, 0x55, 0]);
        let results = PrewarmResults::default();
        results.publish(2, 0, speculate(parent.clone(), tx(2)));
        for value in 1..=MAX_REFRESHES {
            let writes =
                [StorageWrite { address: CONTRACT, slot: U256::ZERO, value: U256::from(value) }];
            results.ready.lock().refresh.replace(0, &writes);
            assert!(results.take_refresh(0, |_| true).is_some());
        }
        results
            .ready
            .lock()
            .refresh
            .replace(0, &[StorageWrite { address: CONTRACT, slot: U256::ZERO, value: U256::MAX }]);
        assert!(results.take_refresh(0, |_| true).is_none());
        let mut predictions = RefreshIndex::default();
        for index in 0..=64 {
            predictions.replace(
                index,
                &[StorageWrite { address: CONTRACT, slot: U256::ZERO, value: U256::from(index) }],
            );
        }
        assert!(predictions.disabled);
        assert!(predictions.writes.is_empty());
    }

    #[test]
    fn refresh_never_waits_on_or_republishes_consumed_results() {
        let mut parent = database();
        contract(&mut parent, &[0x60, 0, 0x54, 0x60, 1, 0x01, 0x60, 0, 0x55, 0]);
        let results = PrewarmResults::default();
        results.publish(0, 0, speculate(parent.clone(), tx(1)));
        results.publish(2, 0, speculate(parent.clone(), tx(2)));
        assert!(results.take_refresh(1, |_| true).is_some());
        assert!(results.take(2).is_some());
        results.publish(2, 0, speculate(parent.clone(), tx(2)));
        assert!(results.take(2).is_none());
        assert!(results.take_refresh(0, |_| true).is_none());
    }

    #[test]
    fn missing_refresh_source_does_not_spend_budget_or_generation() {
        let mut parent = database();
        contract(&mut parent, &[0x60, 0, 0x54, 0x60, 1, 0x01, 0x60, 0, 0x55, 0]);
        let results = PrewarmResults::default();
        results.publish(0, 0, speculate(parent.clone(), tx(1)));
        results.publish(2, 0, speculate(parent, tx(2)));
        assert!(results.take_refresh(0, |_| false).is_none());
        assert_eq!(results.ready.lock().refresh.attempts, 0);
        assert_eq!(results.take_refresh(0, |index| index == 2).unwrap().0, 2);
        assert_eq!(results.ready.lock().refresh.attempts, 1);
    }
}

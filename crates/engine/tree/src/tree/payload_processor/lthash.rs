//! Streaming Lthash accumulator task.

#![allow(dead_code)]

use crate::tree::payload_processor::{
    multiproof::{StateHookSender, StateRootMessage},
    state_io_pool::{StateIoPool, StateIoReadResult},
};
use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_primitives::{keccak256, Address, StorageKey, StorageValue, B256};
use crossbeam_channel::{Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use rayon::prelude::*;
use reth_chain_state::{
    lthash_account_element, lthash_storage_element, LthashAccumulator, LTHASH_ACCOUNT_ELEMENT_LEN,
    LTHASH_STORAGE_ELEMENT_LEN,
};
use reth_evm::OnStateHook;
use reth_primitives_traits::Account;
use reth_provider::ProviderError;
use reth_tasks::Runtime;
use revm_state::EvmState;
use std::{
    collections::HashMap,
    sync::{mpsc, Arc},
};
use tracing::debug_span;

const LTHASH_PARALLEL_EXPAND_THRESHOLD: usize = 64;
const LTHASH_DIRTY_BATCH_SIZE: usize = 256;

/// Message streamed into the Lthash task.
#[derive(Debug, Clone)]
pub(crate) enum LthashMessage {
    AccountTouched {
        address: Address,
        hashed_address: B256,
        new_account: Option<Account>,
    },
    StorageTouched {
        address: Address,
        hashed_address: B256,
        slot: StorageKey,
        hashed_slot: B256,
        new_value: StorageValue,
    },
    FinishedUpdates,
}

/// Result produced by the Lthash task.
#[derive(Debug, Clone)]
pub(crate) struct LthashOutcome {
    pub root: B256,
    pub accumulator: Arc<LthashAccumulator>,
    pub account_updates: usize,
    pub storage_updates: usize,
}

/// Lthash task failure.
#[derive(Debug, thiserror::Error)]
pub(crate) enum LthashError {
    #[error("lthash task dropped without returning an outcome")]
    OutcomeClosed,
    #[error("lthash update stream closed before FinishedUpdates")]
    UpdatesClosed,
    #[error("lthash old-value stream closed with {pending} pending reads")]
    OldValuesClosed { pending: usize },
    #[error("lthash old account read failed for {hashed_address}")]
    AccountRead { hashed_address: B256, source: ProviderError },
    #[error("lthash old storage read failed for {hashed_address}/{hashed_slot}")]
    StorageRead { hashed_address: B256, hashed_slot: B256, source: ProviderError },
    #[error("lthash received old account result for an unknown account {0}")]
    UnknownOldAccount(B256),
    #[error("lthash received duplicate old account result for {0}")]
    DuplicateOldAccount(B256),
    #[error(
        "lthash received old storage result for an unknown slot {hashed_address}/{hashed_slot}"
    )]
    UnknownOldStorage { hashed_address: B256, hashed_slot: B256 },
    #[error("lthash received duplicate old storage result for {hashed_address}/{hashed_slot}")]
    DuplicateOldStorage { hashed_address: B256, hashed_slot: B256 },
}

/// Handle used by payload processing to feed and await the Lthash task.
#[derive(Debug)]
pub(crate) struct LthashHandle {
    updates_tx: CrossbeamSender<LthashMessage>,
    outcome_rx: mpsc::Receiver<Result<LthashOutcome, LthashError>>,
}

impl LthashHandle {
    pub(crate) fn updates_tx(&self) -> CrossbeamSender<LthashMessage> {
        self.updates_tx.clone()
    }

    pub(crate) fn finish_updates(&self) {
        let _ = self.updates_tx.send(LthashMessage::FinishedUpdates);
    }

    pub(crate) fn outcome(self) -> Result<LthashOutcome, LthashError> {
        self.outcome_rx.recv().map_err(|_| LthashError::OutcomeClosed)?
    }
}

/// State hook used by the normal execution path.
///
/// Sparse trie and Lthash consume the same per-transaction state stream. The sparse trie needs the
/// owned [`EvmState`]; Lthash only needs to inspect it and emit its own compact messages.
pub(crate) struct PayloadStateHook {
    sparse: Option<StateHookSender>,
    lthash: Option<CrossbeamSender<LthashMessage>>,
}

impl PayloadStateHook {
    pub(crate) const fn new(
        sparse: Option<StateHookSender>,
        lthash: Option<CrossbeamSender<LthashMessage>>,
    ) -> Self {
        Self { sparse, lthash }
    }
}

impl OnStateHook for PayloadStateHook {
    fn on_state(&mut self, state: EvmState) {
        if let Some(lthash) = self.lthash.as_ref() {
            send_evm_state_to_lthash(&state, lthash);
        }

        if let Some(sparse) = self.sparse.as_ref() {
            let _ = sparse.send(StateRootMessage::StateUpdate(state));
        }
    }
}

impl Drop for PayloadStateHook {
    fn drop(&mut self) {
        if let Some(lthash) = self.lthash.take() {
            let _ = lthash.send(LthashMessage::FinishedUpdates);
        }
    }
}

/// Sends Lthash updates derived from one committed EVM state update.
pub(crate) fn send_evm_state_to_lthash(
    update: &EvmState,
    to_lthash_task: &CrossbeamSender<LthashMessage>,
) {
    for (address, account) in update {
        if !account.is_touched() {
            continue
        }

        let hashed_address = keccak256(address);
        if account.info != account.original_info() {
            let new_account =
                (!account.is_selfdestructed()).then(|| Account::from_revm_account(account));
            let _ = to_lthash_task.send(LthashMessage::AccountTouched {
                address: *address,
                hashed_address,
                new_account,
            });
        }

        if account.is_selfdestructed() {
            continue
        }

        for (slot, value) in account.changed_storage_slots() {
            let slot = *slot;
            let hashed_slot = keccak256(slot.to_be_bytes::<32>());
            let _ = to_lthash_task.send(LthashMessage::StorageTouched {
                address: *address,
                hashed_address,
                slot: slot.into(),
                hashed_slot,
                new_value: value.present_value,
            });
        }
    }
}

/// Sends Lthash storage updates derived from one BAL account entry.
pub(crate) fn send_bal_storage_to_lthash(
    account_changes: &alloy_eip7928::AccountChanges,
    to_lthash_task: &CrossbeamSender<LthashMessage>,
) {
    let address = account_changes.address;
    let hashed_address = keccak256(address);

    for slot_changes in &account_changes.storage_changes {
        let Some(last_change) = slot_changes.changes.last() else { continue };
        let slot = StorageKey::from(slot_changes.slot);
        let hashed_slot = keccak256(slot_changes.slot.to_be_bytes::<32>());
        let _ = to_lthash_task.send(LthashMessage::StorageTouched {
            address,
            hashed_address,
            slot,
            hashed_slot,
            new_value: last_change.new_value,
        });
    }
}

/// Builds the final account value implied by one BAL account entry.
///
/// BAL only carries fields that changed. Missing fields are copied from the parent account.
pub(crate) fn lthash_account_from_bal(
    account_changes: &alloy_eip7928::AccountChanges,
    existing_account: Option<&Account>,
) -> Option<Account> {
    let balance = account_changes.balance_changes.last().map(|change| change.post_balance);
    let nonce = account_changes.nonce_changes.last().map(|change| change.new_nonce);
    let code_hash = account_changes.code_changes.last().map(|code_change| {
        if code_change.new_code.is_empty() {
            KECCAK_EMPTY
        } else {
            keccak256(&code_change.new_code)
        }
    });

    if balance.is_none() && nonce.is_none() && code_hash.is_none() {
        return None
    }

    Some(Account {
        balance: balance
            .unwrap_or_else(|| existing_account.map(|account| account.balance).unwrap_or_default()),
        nonce: nonce.unwrap_or_else(|| existing_account.map(|account| account.nonce).unwrap_or(0)),
        bytecode_hash: code_hash.or_else(|| {
            existing_account.and_then(|account| account.bytecode_hash).or(Some(KECCAK_EMPTY))
        }),
    })
}

/// Spawns a Lthash task and returns its update handle plus the old-value result sender to install
/// into [`StateIoPool::begin_block`](super::state_io_pool::StateIoPool::begin_block).
pub(crate) fn spawn_lthash_task(
    executor: &Runtime,
    parent_accumulator: LthashAccumulator,
    state_io_pool: Arc<StateIoPool>,
) -> (LthashHandle, CrossbeamSender<StateIoReadResult>) {
    let (updates_tx, updates_rx) = crossbeam_channel::unbounded();
    let (old_values_tx, old_values_rx) = crossbeam_channel::unbounded();
    let (outcome_tx, outcome_rx) = mpsc::channel();

    executor.spawn_blocking_named("lthash", move || {
        let task = LthashTask::new(parent_accumulator, updates_rx, old_values_rx, state_io_pool);
        let _ = outcome_tx.send(task.run());
    });

    (LthashHandle { updates_tx, outcome_rx }, old_values_tx)
}

/// Streaming task that maintains the block's Lthash accumulator.
pub(crate) struct LthashTask {
    updates: CrossbeamReceiver<LthashMessage>,
    old_values: CrossbeamReceiver<StateIoReadResult>,
    state_io_pool: Arc<StateIoPool>,
    state: LthashState,
}

impl LthashTask {
    pub(crate) fn new(
        parent_accumulator: LthashAccumulator,
        updates: CrossbeamReceiver<LthashMessage>,
        old_values: CrossbeamReceiver<StateIoReadResult>,
        state_io_pool: Arc<StateIoPool>,
    ) -> Self {
        Self { updates, old_values, state_io_pool, state: LthashState::new(parent_accumulator) }
    }

    pub(crate) fn run(mut self) -> Result<LthashOutcome, LthashError> {
        let mut finished = false;

        while !finished {
            let mut did_work = false;

            while let Ok(message) = self.updates.try_recv() {
                did_work = true;
                if self.handle_message(message)? {
                    finished = true;
                    break
                }
            }

            while let Ok(result) = self.old_values.try_recv() {
                did_work = true;
                self.state.apply_old_result(result)?;
            }

            if finished {
                break
            }

            if !did_work && self.state.process_dirty_batch(LTHASH_DIRTY_BATCH_SIZE) > 0 {
                continue
            }

            if !did_work {
                crossbeam_channel::select! {
                    recv(self.updates) -> message => {
                        let message = match message {
                            Ok(message) => message,
                            Err(_) => {
                                tracing::debug!(
                                    target: "engine::tree::payload_processor::lthash",
                                    pending_old_reads = self.state.pending_old_reads(),
                                    "lthash update channel closed while blocked"
                                );
                                return Err(LthashError::UpdatesClosed)
                            }
                        };
                        tracing::debug!(
                            target: "engine::tree::payload_processor::lthash",
                            kind = lthash_message_kind(&message),
                            pending_old_reads = self.state.pending_old_reads(),
                            "lthash blocked select received update"
                        );
                        if self.handle_message(message)? {
                            finished = true;
                        }
                    }
                    recv(self.old_values) -> result => {
                        match result {
                            Ok(result) => {
                                tracing::debug!(
                                    target: "engine::tree::payload_processor::lthash",
                                    kind = old_value_result_kind(&result),
                                    error = old_value_result_is_error(&result),
                                    pending_old_reads = self.state.pending_old_reads(),
                                    "lthash blocked select received old value"
                                );
                                self.state.apply_old_result(result)?
                            }
                            Err(_) if self.state.pending_old_reads() == 0 => {
                                tracing::debug!(
                                    target: "engine::tree::payload_processor::lthash",
                                    pending_old_reads = 0,
                                    "lthash old-value channel closed while blocked"
                                );
                            }
                            Err(_) => {
                                tracing::debug!(
                                    target: "engine::tree::payload_processor::lthash",
                                    pending_old_reads = self.state.pending_old_reads(),
                                    "lthash old-value channel closed with pending reads"
                                );
                                return Err(LthashError::OldValuesClosed {
                                    pending: self.state.pending_old_reads(),
                                });
                            }
                        }
                    }
                }
            }
        }

        self.state_io_pool.end_block().wait();
        self.drain_pending_old_values()?;
        self.state.process_all_dirty();
        Ok(self.state.finish())
    }

    fn handle_message(&mut self, message: LthashMessage) -> Result<bool, LthashError> {
        match message {
            LthashMessage::AccountTouched { address, hashed_address, new_account } => {
                self.state.touch_account(
                    address,
                    hashed_address,
                    new_account,
                    |address, hashed| {
                        self.state_io_pool.read_account(address, hashed);
                    },
                );
                Ok(false)
            }
            LthashMessage::StorageTouched {
                address,
                hashed_address,
                slot,
                hashed_slot,
                new_value,
            } => {
                self.state.touch_storage(
                    address,
                    hashed_address,
                    slot,
                    hashed_slot,
                    new_value,
                    |address, hashed_address, slot, hashed_slot| {
                        self.state_io_pool.read_storage(address, hashed_address, slot, hashed_slot);
                    },
                );
                Ok(false)
            }
            LthashMessage::FinishedUpdates => Ok(true),
        }
    }

    fn drain_pending_old_values(&mut self) -> Result<(), LthashError> {
        while self.state.pending_old_reads() > 0 {
            let result = self.old_values.recv().map_err(|_| LthashError::OldValuesClosed {
                pending: self.state.pending_old_reads(),
            })?;
            self.state.apply_old_result(result)?;
        }

        while let Ok(result) = self.old_values.try_recv() {
            self.state.apply_old_result(result)?;
        }

        Ok(())
    }
}

fn lthash_message_kind(message: &LthashMessage) -> &'static str {
    match message {
        LthashMessage::AccountTouched { .. } => "account",
        LthashMessage::StorageTouched { .. } => "storage",
        LthashMessage::FinishedUpdates => "finished",
    }
}

fn old_value_result_kind(result: &StateIoReadResult) -> &'static str {
    match result {
        StateIoReadResult::Account { .. } => "account",
        StateIoReadResult::Storage { .. } => "storage",
    }
}

fn old_value_result_is_error(result: &StateIoReadResult) -> bool {
    match result {
        StateIoReadResult::Account { account, .. } => account.is_err(),
        StateIoReadResult::Storage { value, .. } => value.is_err(),
    }
}

#[derive(Debug)]
struct LthashState {
    accumulator: LthashAccumulator,
    accounts: HashMap<B256, AccountEntry>,
    storages: HashMap<(B256, B256), StorageEntry>,
    pending_old_reads: usize,
    account_updates: usize,
    storage_updates: usize,
}

impl LthashState {
    fn new(accumulator: LthashAccumulator) -> Self {
        Self {
            accumulator,
            accounts: HashMap::default(),
            storages: HashMap::default(),
            pending_old_reads: 0,
            account_updates: 0,
            storage_updates: 0,
        }
    }

    fn pending_old_reads(&self) -> usize {
        self.pending_old_reads
    }

    fn touch_account(
        &mut self,
        address: Address,
        hashed_address: B256,
        new_account: Option<Account>,
        schedule_old_read: impl FnOnce(Address, B256),
    ) {
        match self.accounts.get_mut(&hashed_address) {
            Some(entry) => {
                if let Some(element) = entry.added_new.take() {
                    self.accumulator.subtract(element);
                }
                entry.latest_new = new_account;
                entry.dirty_new = true;
            }
            None => {
                self.accounts.insert(
                    hashed_address,
                    AccountEntry {
                        address,
                        old_done: false,
                        latest_new: new_account,
                        added_new: None,
                        dirty_new: true,
                    },
                );
                self.pending_old_reads += 1;
                self.account_updates += 1;
                schedule_old_read(address, hashed_address);
            }
        }
    }

    fn touch_storage(
        &mut self,
        address: Address,
        hashed_address: B256,
        slot: StorageKey,
        hashed_slot: B256,
        new_value: StorageValue,
        schedule_old_read: impl FnOnce(Address, B256, StorageKey, B256),
    ) {
        let key = (hashed_address, hashed_slot);
        match self.storages.get_mut(&key) {
            Some(entry) => {
                if let Some(element) = entry.added_new.take() {
                    self.accumulator.subtract(element);
                }
                entry.latest_new = new_value;
                entry.dirty_new = true;
            }
            None => {
                self.storages.insert(
                    key,
                    StorageEntry {
                        address,
                        slot,
                        old_done: false,
                        latest_new: new_value,
                        added_new: None,
                        dirty_new: true,
                    },
                );
                self.pending_old_reads += 1;
                self.storage_updates += 1;
                schedule_old_read(address, hashed_address, slot, hashed_slot);
            }
        }
    }

    fn apply_old_result(&mut self, result: StateIoReadResult) -> Result<(), LthashError> {
        match result {
            StateIoReadResult::Account { hashed_address, account } => {
                self.apply_old_account(hashed_address, account)
            }
            StateIoReadResult::Storage { hashed_address, hashed_slot, value } => {
                self.apply_old_storage(hashed_address, hashed_slot, value)
            }
        }
    }

    fn apply_old_account(
        &mut self,
        hashed_address: B256,
        account: Result<Option<Account>, ProviderError>,
    ) -> Result<(), LthashError> {
        let entry = self
            .accounts
            .get_mut(&hashed_address)
            .ok_or(LthashError::UnknownOldAccount(hashed_address))?;
        if entry.old_done {
            return Err(LthashError::DuplicateOldAccount(hashed_address))
        }

        let account =
            account.map_err(|source| LthashError::AccountRead { hashed_address, source })?;
        if let Some(account) = account &&
            let Some(element) = lthash_account_element(hashed_address, account)
        {
            self.accumulator.subtract(element);
        }

        entry.old_done = true;
        self.pending_old_reads -= 1;
        Ok(())
    }

    fn apply_old_storage(
        &mut self,
        hashed_address: B256,
        hashed_slot: B256,
        value: Result<StorageValue, ProviderError>,
    ) -> Result<(), LthashError> {
        let entry = self
            .storages
            .get_mut(&(hashed_address, hashed_slot))
            .ok_or(LthashError::UnknownOldStorage { hashed_address, hashed_slot })?;
        if entry.old_done {
            return Err(LthashError::DuplicateOldStorage { hashed_address, hashed_slot })
        }

        let value = value.map_err(|source| LthashError::StorageRead {
            hashed_address,
            hashed_slot,
            source,
        })?;
        if let Some(element) = lthash_storage_element(hashed_address, hashed_slot, value) {
            self.accumulator.subtract(element);
        }

        entry.old_done = true;
        self.pending_old_reads -= 1;
        Ok(())
    }

    fn process_dirty_batch(&mut self, max: usize) -> usize {
        let mut processed = 0;
        processed += self.process_dirty_accounts(max.saturating_sub(processed));
        if processed < max {
            processed += self.process_dirty_storages(max - processed);
        }
        processed
    }

    fn process_all_dirty(&mut self) {
        while self.process_dirty_batch(usize::MAX) > 0 {}
    }

    fn process_dirty_accounts(&mut self, max: usize) -> usize {
        if max == 0 {
            return 0
        }

        let span = debug_span!(
            target: "engine::tree::payload_processor::lthash",
            "process_dirty_accounts",
            max,
            tracked = self.accounts.len(),
            pending_old_reads = self.pending_old_reads,
            dirty = tracing::field::Empty,
            elements = tracing::field::Empty,
        )
        .entered();

        let mut dirty = Vec::new();
        for (&hashed_address, entry) in &mut self.accounts {
            if dirty.len() == max {
                break
            }
            if !entry.dirty_new {
                continue
            }

            entry.dirty_new = false;
            let element = entry
                .latest_new
                .and_then(|account| lthash_account_element(hashed_address, account));
            dirty.push((hashed_address, element));
        }

        let elements = dirty.iter().filter_map(|(_, element)| *element).collect::<Vec<_>>();
        span.record("dirty", dirty.len());
        span.record("elements", elements.len());
        add_account_elements(&mut self.accumulator, &elements);

        for (hashed_address, element) in dirty.iter().copied() {
            if let Some(entry) = self.accounts.get_mut(&hashed_address) {
                entry.added_new = element;
            }
        }

        dirty.len()
    }

    fn process_dirty_storages(&mut self, max: usize) -> usize {
        if max == 0 {
            return 0
        }

        let span = debug_span!(
            target: "engine::tree::payload_processor::lthash",
            "process_dirty_storages",
            max,
            tracked = self.storages.len(),
            pending_old_reads = self.pending_old_reads,
            dirty = tracing::field::Empty,
            elements = tracing::field::Empty,
        )
        .entered();

        let mut dirty = Vec::new();
        for (&(hashed_address, hashed_slot), entry) in &mut self.storages {
            if dirty.len() == max {
                break
            }
            if !entry.dirty_new {
                continue
            }

            entry.dirty_new = false;
            let element = lthash_storage_element(hashed_address, hashed_slot, entry.latest_new);
            dirty.push(((hashed_address, hashed_slot), element));
        }

        let elements = dirty.iter().filter_map(|(_, element)| *element).collect::<Vec<_>>();
        span.record("dirty", dirty.len());
        span.record("elements", elements.len());
        add_storage_elements(&mut self.accumulator, &elements);

        for ((hashed_address, hashed_slot), element) in dirty.iter().copied() {
            if let Some(entry) = self.storages.get_mut(&(hashed_address, hashed_slot)) {
                entry.added_new = element;
            }
        }

        dirty.len()
    }

    fn finish(self) -> LthashOutcome {
        let accumulator = Arc::new(self.accumulator);
        LthashOutcome {
            root: accumulator.checksum(),
            accumulator,
            account_updates: self.account_updates,
            storage_updates: self.storage_updates,
        }
    }
}

#[derive(Debug)]
struct AccountEntry {
    address: Address,
    old_done: bool,
    latest_new: Option<Account>,
    added_new: Option<[u8; LTHASH_ACCOUNT_ELEMENT_LEN]>,
    dirty_new: bool,
}

#[derive(Debug)]
struct StorageEntry {
    address: Address,
    slot: StorageKey,
    old_done: bool,
    latest_new: StorageValue,
    added_new: Option<[u8; LTHASH_STORAGE_ELEMENT_LEN]>,
    dirty_new: bool,
}

fn add_account_elements(
    accumulator: &mut LthashAccumulator,
    elements: &[[u8; LTHASH_ACCOUNT_ELEMENT_LEN]],
) {
    let _span = debug_span!(
        target: "engine::tree::payload_processor::lthash",
        "add_account_elements",
        elements = elements.len(),
        parallel = elements.len() >= LTHASH_PARALLEL_EXPAND_THRESHOLD,
        threshold = LTHASH_PARALLEL_EXPAND_THRESHOLD,
    )
    .entered();
    add_elements(accumulator, elements);
}

fn add_storage_elements(
    accumulator: &mut LthashAccumulator,
    elements: &[[u8; LTHASH_STORAGE_ELEMENT_LEN]],
) {
    let _span = debug_span!(
        target: "engine::tree::payload_processor::lthash",
        "add_storage_elements",
        elements = elements.len(),
        parallel = elements.len() >= LTHASH_PARALLEL_EXPAND_THRESHOLD,
        threshold = LTHASH_PARALLEL_EXPAND_THRESHOLD,
    )
    .entered();
    add_elements(accumulator, elements);
}

fn add_elements<const N: usize>(accumulator: &mut LthashAccumulator, elements: &[[u8; N]]) {
    if elements.len() >= LTHASH_PARALLEL_EXPAND_THRESHOLD {
        let delta = elements
            .par_iter()
            .map(|element| {
                let mut accumulator = LthashAccumulator::zero();
                accumulator.add(element);
                accumulator
            })
            .reduce(LthashAccumulator::zero, |mut left, right| {
                left.combine(&right);
                left
            });
        accumulator.combine(&delta);
    } else {
        for element in elements {
            accumulator.add(element);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{b256, U256};
    use reth_execution_cache::ExecutionCache;

    use crate::tree::payload_processor::state_io_pool::BuildProviderFn;

    fn account(nonce: u64, balance: u64) -> Account {
        Account { nonce, balance: U256::from(balance), bytecode_hash: None }
    }

    #[test]
    fn replacement_uses_only_old_and_latest_new_values() {
        let hashed_address =
            b256!("0x0101010101010101010101010101010101010101010101010101010101010101");
        let old = account(1, 10);
        let mid = account(2, 20);
        let new = account(3, 30);

        let mut parent = LthashAccumulator::zero();
        parent.add(lthash_account_element(hashed_address, old).unwrap());

        let mut state = LthashState::new(parent);
        let mut scheduled = Vec::new();
        state.touch_account(Address::ZERO, hashed_address, Some(mid), |address, hashed| {
            scheduled.push((address, hashed));
        });
        assert_eq!(scheduled, vec![(Address::ZERO, hashed_address)]);

        state.process_all_dirty();
        state.touch_account(Address::ZERO, hashed_address, Some(new), |_, _| {
            panic!("old account read must only be scheduled once")
        });
        state
            .apply_old_result(StateIoReadResult::Account { hashed_address, account: Ok(Some(old)) })
            .unwrap();
        state.process_all_dirty();

        let outcome = state.finish();
        let mut expected = LthashAccumulator::zero();
        expected.add(lthash_account_element(hashed_address, new).unwrap());

        assert_eq!(&*outcome.accumulator, &expected);
        assert_eq!(outcome.root, expected.checksum());
        assert_eq!(outcome.account_updates, 1);
        assert_eq!(outcome.storage_updates, 0);
    }

    #[test]
    fn storage_zero_removes_latest_element() {
        let hashed_address = B256::repeat_byte(0x11);
        let hashed_slot = B256::repeat_byte(0x22);
        let old = U256::from(7);

        let mut parent = LthashAccumulator::zero();
        parent.add(lthash_storage_element(hashed_address, hashed_slot, old).unwrap());

        let mut state = LthashState::new(parent);
        state.touch_storage(
            Address::ZERO,
            hashed_address,
            StorageKey::ZERO,
            hashed_slot,
            U256::ZERO,
            |_, _, _, _| {},
        );
        state
            .apply_old_result(StateIoReadResult::Storage {
                hashed_address,
                hashed_slot,
                value: Ok(old),
            })
            .unwrap();
        state.process_all_dirty();

        assert!(state.finish().accumulator.is_zero());
    }

    #[test]
    fn old_read_error_fails_the_task_state() {
        let hashed_address = B256::repeat_byte(0x33);
        let mut state = LthashState::new(LthashAccumulator::zero());
        state.touch_account(Address::ZERO, hashed_address, Some(account(1, 1)), |_, _| {});

        let err = state
            .apply_old_result(StateIoReadResult::Account {
                hashed_address,
                account: Err(ProviderError::UnsupportedProvider),
            })
            .unwrap_err();

        assert!(
            matches!(err, LthashError::AccountRead { hashed_address: h, .. } if h == hashed_address)
        );
        assert_eq!(state.pending_old_reads(), 1);
    }

    #[test]
    fn parallel_batch_matches_serial_addition() {
        let elements = (0..(LTHASH_PARALLEL_EXPAND_THRESHOLD + 1))
            .map(|i| {
                let hashed_address = B256::from(U256::from(i).to_be_bytes::<32>());
                lthash_account_element(hashed_address, account(i as u64 + 1, i as u64 + 2)).unwrap()
            })
            .collect::<Vec<_>>();

        let mut parallel = LthashAccumulator::zero();
        add_account_elements(&mut parallel, &elements);

        let mut serial = LthashAccumulator::zero();
        for element in &elements {
            serial.add(element);
        }

        assert_eq!(parallel, serial);
    }

    #[test]
    fn task_finishes_empty_block() {
        let (updates_tx, updates_rx) = crossbeam_channel::unbounded();
        let (_old_values_tx, old_values_rx) = crossbeam_channel::unbounded();
        let pool = StateIoPool::new(1);
        let task = LthashTask::new(LthashAccumulator::zero(), updates_rx, old_values_rx, pool);

        updates_tx.send(LthashMessage::FinishedUpdates).unwrap();
        let outcome = task.run().unwrap();

        assert!(outcome.accumulator.is_zero());
        assert_eq!(outcome.root, LthashAccumulator::zero().checksum());
        assert_eq!(outcome.account_updates, 0);
        assert_eq!(outcome.storage_updates, 0);
    }

    #[test]
    fn task_surfaces_state_io_read_errors() {
        let (updates_tx, updates_rx) = crossbeam_channel::unbounded();
        let (old_values_tx, old_values_rx) = crossbeam_channel::unbounded();
        let pool = StateIoPool::new(1);
        let build: Arc<BuildProviderFn> = Arc::new(|| Err(ProviderError::UnsupportedProvider));
        pool.begin_block(build, ExecutionCache::new(1), Some(old_values_tx));

        let task =
            LthashTask::new(LthashAccumulator::zero(), updates_rx, old_values_rx, pool.clone());
        let hashed_address = B256::repeat_byte(0x44);

        updates_tx
            .send(LthashMessage::AccountTouched {
                address: Address::ZERO,
                hashed_address,
                new_account: Some(account(1, 1)),
            })
            .unwrap();
        updates_tx.send(LthashMessage::FinishedUpdates).unwrap();

        let err = task.run().unwrap_err();
        assert!(
            matches!(err, LthashError::AccountRead { hashed_address: h, .. } if h == hashed_address)
        );
    }
}

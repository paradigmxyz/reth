//! Experimental helpers for parallel clean state trie rebuilds.

mod account_prefix;
mod config;
mod stats;
mod storage_prefix;

#[cfg(test)]
use account_prefix::account_prefix_bound;
use account_prefix::{
    account_prefix_last_key, plan_account_prefixes_window, AccountPrefixBarrier,
    AccountPrefixBuildState, AccountPrefixPlan, AccountPrefixPlanItem, AccountPrefixRange,
    AccountPrefixResult, AccountPrefixSingle, AccountPrefixStorageHint,
};
use alloy_primitives::{B256, U256};
use alloy_rlp::{BufMut, Encodable};
use reth_execution_errors::StorageRootError;
use reth_primitives_traits::Account;
use reth_storage_errors::db::DatabaseError;
use reth_trie::{
    hashed_cursor::{HashedCursor, HashedCursorFactory, HashedStorageKeyCursor},
    prefix_set::PrefixSet,
    trie_cursor::noop::NoopTrieCursorFactory,
    updates::{StorageTrieUpdates, TrieUpdates},
    HashBuilder, IntermediateRootState, IntermediateStateRootState, IntermediateStorageRootState,
    Nibbles, StateRootProgress, StorageRoot, StorageRootProgress, EMPTY_ROOT_HASH,
    TRIE_ACCOUNT_RLP_MAX_SIZE,
};
use std::sync::mpsc;
#[cfg(test)]
use storage_prefix::plan_storage_prefixes;
use storage_prefix::{
    plan_storage_prefixes_after, storage_prefix_bound, storage_prefix_end,
    SegmentedStorageUpdateEstimator, StoragePrefixPlan, StoragePrefixPlanBudget,
    StoragePrefixRange,
};
use thiserror::Error;

pub use config::StorageRootPrefetchConfig;
pub use stats::StorageRootPrefetchStats;
pub use storage_prefix::StoragePrefixPlannerConfig;

/// Experimental full-rebuild state root calculator.
///
/// This is intentionally scoped to clean rebuild semantics: storage roots are computed against an
/// empty trie cursor and only use the hashed storage table. The account trie is still folded on the
/// caller thread in account hash order.
#[derive(Debug)]
pub struct ParallelRebuildStateRoot<H> {
    hashed_cursor_factory: H,
    config: StorageRootPrefetchConfig,
    previous_state: Option<IntermediateStateRootState>,
}

impl<H> ParallelRebuildStateRoot<H> {
    /// Creates a new clean rebuild state root calculator.
    pub fn new(hashed_cursor_factory: H) -> Self {
        Self {
            hashed_cursor_factory,
            config: StorageRootPrefetchConfig::default(),
            previous_state: None,
        }
    }

    /// Sets the storage root prefetch configuration.
    pub const fn with_config(mut self, config: StorageRootPrefetchConfig) -> Self {
        self.config = config;
        self
    }

    /// Sets the previously recorded intermediate state.
    pub fn with_intermediate_state(mut self, state: Option<IntermediateStateRootState>) -> Self {
        self.previous_state = state;
        self
    }
}

impl<H> ParallelRebuildStateRoot<H>
where
    H: HashedCursorFactory + Clone + Send + Sync,
{
    /// Calculates the state root progress, returning helper statistics.
    pub fn root_with_progress_and_stats(
        mut self,
    ) -> Result<ParallelRebuildStateRootProgressOutcome, ParallelRebuildStateRootError> {
        let depth = self.config.account_prefix_max_depth;
        if depth == 0 {
            return Err(ParallelRebuildStateRootError::AccountPrefixRebuildDisabled)
        }
        calculate_storage_aware_account_prefix_rebuild(
            self.hashed_cursor_factory,
            depth,
            self.config,
            self.previous_state.take(),
        )
    }
}

/// Output of [`ParallelRebuildStateRoot::root_with_progress_and_stats`].
#[derive(Debug)]
pub struct ParallelRebuildStateRootProgressOutcome {
    /// State root progress.
    pub progress: StateRootProgress,
    /// Storage prefetch helper statistics.
    pub prefetch: StorageRootPrefetchStats,
}

struct StoragePresenceCursor<'a> {
    cursor: Box<dyn HashedStorageKeyCursor + 'a>,
    next_key: Option<B256>,
}

impl<'a> StoragePresenceCursor<'a> {
    fn from_start(
        mut cursor: Box<dyn HashedStorageKeyCursor + 'a>,
        start_account_key: B256,
    ) -> Result<Self, DatabaseError> {
        let next_key = cursor.seek_storage_key(start_account_key)?;
        Ok(Self { cursor, next_key })
    }

    fn lookup(
        &mut self,
        hashed_address: B256,
    ) -> Result<(AccountStoragePresence, usize), DatabaseError> {
        let skipped_keys = self.skip_before(hashed_address)?;

        if self.next_key == Some(hashed_address) {
            let slots = self.cursor.current_storage_entry_count()?;
            return Ok((AccountStoragePresence::Present { slots }, skipped_keys))
        }

        Ok((AccountStoragePresence::Empty, skipped_keys))
    }

    fn skip_before(&mut self, hashed_address: B256) -> Result<usize, DatabaseError> {
        let mut skipped_keys = 0usize;
        while self.next_key.is_some_and(|storage_key| storage_key < hashed_address) {
            self.next_key = self.cursor.next_storage_key()?;
            skipped_keys += 1;
        }
        Ok(skipped_keys)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccountStoragePresence {
    Empty,
    Present { slots: Option<usize> },
}

fn create_progress(
    hash_builder: HashBuilder,
    last_hashed_key: B256,
    hashed_entries_walked: usize,
    mut trie_updates: TrieUpdates,
    storage_root_state: Option<IntermediateStorageRootState>,
) -> StateRootProgress {
    let (hash_builder, hash_builder_updates) = hash_builder.split();
    trie_updates.account_nodes.extend(hash_builder_updates);

    StateRootProgress::Progress(
        Box::new(IntermediateStateRootState {
            account_root_state: IntermediateRootState {
                hash_builder,
                walker_stack: Vec::new(),
                last_hashed_key,
            },
            storage_root_state,
        }),
        hashed_entries_walked,
        trie_updates,
    )
}

#[derive(Debug)]
struct StorageRootJobOutcome {
    progress: StorageRootProgress,
    stats: StorageRootPrefetchStats,
}

#[derive(Debug)]
struct InlineStorageRootJob {
    job: StorageRootJobOutcome,
    walked: usize,
}

fn inline_empty_storage_root_job() -> InlineStorageRootJob {
    InlineStorageRootJob {
        walked: 0,
        job: StorageRootJobOutcome {
            progress: StorageRootProgress::Complete(
                EMPTY_ROOT_HASH,
                0,
                StorageTrieUpdates::deleted(),
            ),
            stats: StorageRootPrefetchStats::default(),
        },
    }
}

fn inline_storage_root_from_slots(
    slots: Vec<(B256, alloy_primitives::U256)>,
) -> InlineStorageRootJob {
    if slots.is_empty() {
        return inline_empty_storage_root_job()
    }

    let walked = slots.len();
    let mut hash_builder = HashBuilder::default().with_updates(true);
    for (hashed_slot, value) in slots {
        hash_builder
            .add_leaf(Nibbles::unpack(hashed_slot), alloy_rlp::encode_fixed_size(&value).as_ref());
    }
    let root = hash_builder.root();
    let mut updates = StorageTrieUpdates::default();
    updates.finalize(hash_builder, Default::default());

    InlineStorageRootJob {
        walked,
        job: StorageRootJobOutcome {
            progress: StorageRootProgress::Complete(root, walked, updates),
            stats: StorageRootPrefetchStats::default(),
        },
    }
}

fn calculate_storage_aware_account_prefix_rebuild<H>(
    hashed_cursor_factory: H,
    depth: usize,
    config: StorageRootPrefetchConfig,
    previous_state: Option<IntermediateStateRootState>,
) -> Result<ParallelRebuildStateRootProgressOutcome, ParallelRebuildStateRootError>
where
    H: HashedCursorFactory + Clone + Send + Sync,
{
    let last_account_key =
        previous_state.as_ref().map(|state| state.account_root_state.last_hashed_key);
    let worker_count = rayon::current_num_threads().max(1);
    let window_size = if config.account_prefix_window_size == 0 {
        worker_count.saturating_mul(4).max(1)
    } else {
        config.account_prefix_window_size
    };
    let plan = plan_account_prefixes_window(
        &hashed_cursor_factory,
        depth,
        config,
        last_account_key,
        window_size,
    )?;
    calculate_account_prefix_rebuild_with_plan(hashed_cursor_factory, plan, config, previous_state)
}

fn calculate_account_prefix_rebuild_with_plan<H>(
    hashed_cursor_factory: H,
    plan: AccountPrefixPlan,
    config: StorageRootPrefetchConfig,
    previous_state: Option<IntermediateStateRootState>,
) -> Result<ParallelRebuildStateRootProgressOutcome, ParallelRebuildStateRootError>
where
    H: HashedCursorFactory + Clone + Send + Sync,
{
    let mut stats = StorageRootPrefetchStats::default();
    stats.record_account_prefix_plan(&plan.stats);
    let mut hashed_entries_walked = 0usize;
    let mut updates = TrieUpdates::default();
    let (mut hash_builder, mut last_account_key, pending_storage_root) =
        account_prefix_initial_state(previous_state)?;
    if let Some(storage_state) = pending_storage_root {
        let mut execution = AccountPrefixExecution {
            hash_builder: &mut hash_builder,
            updates: &mut updates,
            stats: &mut stats,
            hashed_entries_walked: &mut hashed_entries_walked,
            config,
        };
        let outcome = complete_account_prefix_barrier_storage(
            hashed_cursor_factory.clone(),
            last_account_key.expect("storage checkpoint has an account key"),
            storage_state.account,
            Some(storage_state.state),
            &mut execution,
        )?;
        if let Some(progress) = outcome {
            return Ok(ParallelRebuildStateRootProgressOutcome { progress, prefetch: stats })
        }
        stats.account_prefix_completed_barriers += 1;
    }
    let worker_count = rayon::current_num_threads().max(1);
    let window_size = if config.account_prefix_window_size == 0 {
        worker_count.saturating_mul(4).max(1)
    } else {
        config.account_prefix_window_size
    };
    let plan_complete = plan.complete;
    let remaining_items = plan
        .items
        .into_iter()
        .filter(|item| {
            last_account_key.is_none_or(|last_account_key| item.start() > last_account_key)
        })
        .collect::<Vec<_>>();
    let mut item_index = 0usize;

    while item_index < remaining_items.len() {
        if let AccountPrefixPlanItem::Barrier(barrier) = remaining_items[item_index].clone() {
            let has_more_after_barrier = item_index + 1 < remaining_items.len();
            let mut execution = AccountPrefixExecution {
                hash_builder: &mut hash_builder,
                updates: &mut updates,
                stats: &mut stats,
                hashed_entries_walked: &mut hashed_entries_walked,
                config,
            };
            let outcome = process_account_prefix_barrier(
                hashed_cursor_factory.clone(),
                barrier,
                has_more_after_barrier,
                &mut execution,
            )?;
            last_account_key = Some(barrier.hashed_address);
            if let Some(progress) = outcome {
                return Ok(ParallelRebuildStateRootProgressOutcome { progress, prefetch: stats })
            }
            item_index += 1;
            continue
        }
        if let AccountPrefixPlanItem::Single(single) = remaining_items[item_index].clone() {
            let has_more_after_single = item_index + 1 < remaining_items.len();
            let mut execution = AccountPrefixExecution {
                hash_builder: &mut hash_builder,
                updates: &mut updates,
                stats: &mut stats,
                hashed_entries_walked: &mut hashed_entries_walked,
                config,
            };
            let outcome = process_account_prefix_single(
                hashed_cursor_factory.clone(),
                single,
                has_more_after_single,
                &mut execution,
            )?;
            last_account_key = Some(single.hashed_address);
            if let Some(progress) = outcome {
                return Ok(ParallelRebuildStateRootProgressOutcome { progress, prefetch: stats })
            }
            item_index += 1;
            continue
        }

        let window_end = (item_index..remaining_items.len())
            .take(window_size)
            .take_while(|index| matches!(remaining_items[*index], AccountPrefixPlanItem::Range(_)))
            .last()
            .map_or(item_index, |index| index + 1);
        let window = &remaining_items[item_index..window_end];
        let has_more_after_window = window_end < remaining_items.len();
        stats.account_prefix_windows += 1;
        let mut prefix_results = std::iter::repeat_with(|| None)
            .take(window.len())
            .collect::<Vec<Option<AccountPrefixResult>>>();
        let mut work_items = window
            .iter()
            .cloned()
            .enumerate()
            .filter_map(|(offset, item)| match item {
                AccountPrefixPlanItem::Range(range) => Some((offset, range)),
                AccountPrefixPlanItem::Barrier(_) | AccountPrefixPlanItem::Single(_) => None,
            })
            .collect::<Vec<_>>();
        work_items.sort_unstable_by(|(left_offset, left), (right_offset, right)| {
            right.weight.cmp(&left.weight).then_with(|| left_offset.cmp(right_offset))
        });
        let (tx, rx) = mpsc::sync_channel(work_items.len());

        let window_compute_started = std::time::Instant::now();
        rayon::scope(|scope| {
            for (offset, range) in work_items.iter().cloned() {
                let tx = tx.clone();
                let hashed_cursor_factory = hashed_cursor_factory.clone();
                scope.spawn(move |_| {
                    let result =
                        calculate_account_prefix_root(hashed_cursor_factory, range, config);
                    let _ = tx.send((offset, result));
                });
            }
        });
        drop(tx);

        for _ in 0..work_items.len() {
            let (offset, result) =
                rx.recv().map_err(|_| ParallelRebuildStateRootError::AccountPrefixWorkerDropped)?;
            match result? {
                AccountPrefixOutcome::Complete(result) => {
                    let result = *result;
                    hashed_entries_walked += result.walked_accounts + result.walked_storage_slots;
                    stats.extend(result.stats.clone());
                    prefix_results[offset] = Some(result);
                }
                AccountPrefixOutcome::Empty => {}
                AccountPrefixOutcome::InlineRoot(prefix) => {
                    return Err(ParallelRebuildStateRootError::InlineAccountPrefixRoot(prefix))
                }
                AccountPrefixOutcome::UnsupportedPrefix(prefix) => {
                    return Err(ParallelRebuildStateRootError::UnsupportedAccountPrefix(prefix))
                }
            }
        }
        stats.account_prefix_window_compute_duration += window_compute_started.elapsed();

        let merge_started = std::time::Instant::now();
        for result in prefix_results.into_iter().flatten() {
            last_account_key = Some(result.last_hashed_address);
            let children_are_in_trie = !result.updates.account_nodes.is_empty();
            updates.extend(result.updates);
            hash_builder.add_branch(result.prefix, result.root_hash, children_are_in_trie);
            stats.account_prefix_completed_ranges += 1;
        }
        stats.account_prefix_merge_duration += merge_started.elapsed();

        if has_more_after_window &&
            account_prefix_progress_threshold_reached(&updates, &hash_builder, config) &&
            let Some(last_account_key) = last_account_key
        {
            stats.account_prefix_boundary_checkpoints += 1;
            let progress = create_progress(
                hash_builder,
                last_account_key,
                hashed_entries_walked,
                updates,
                None,
            );
            return Ok(ParallelRebuildStateRootProgressOutcome { progress, prefetch: stats })
        }
        item_index = window_end;
    }

    if !plan_complete && let Some(last_account_key) = last_account_key {
        stats.account_prefix_boundary_checkpoints += 1;
        let progress =
            create_progress(hash_builder, last_account_key, hashed_entries_walked, updates, None);
        return Ok(ParallelRebuildStateRootProgressOutcome { progress, prefetch: stats })
    }

    let finalize_started = std::time::Instant::now();
    let root = hash_builder.root();
    updates.finalize(hash_builder, Default::default(), Default::default());
    stats.state_root_finalize_duration += finalize_started.elapsed();

    Ok(ParallelRebuildStateRootProgressOutcome {
        progress: StateRootProgress::Complete(root, hashed_entries_walked, updates),
        prefetch: stats,
    })
}

fn account_prefix_initial_state(
    previous_state: Option<IntermediateStateRootState>,
) -> Result<
    (HashBuilder, Option<B256>, Option<IntermediateStorageRootState>),
    ParallelRebuildStateRootError,
> {
    let Some(previous_state) = previous_state else {
        return Ok((HashBuilder::default().with_updates(true), None, None))
    };

    let IntermediateStateRootState { account_root_state, storage_root_state } = previous_state;
    if !account_root_state.walker_stack.is_empty() ||
        storage_root_state.as_ref().is_some_and(|state| !state.state.walker_stack.is_empty())
    {
        return Err(ParallelRebuildStateRootError::UnsupportedAccountWalkerCheckpoint)
    }

    Ok((
        account_root_state.hash_builder.with_updates(true),
        Some(account_root_state.last_hashed_key),
        storage_root_state,
    ))
}

struct AccountPrefixExecution<'a> {
    hash_builder: &'a mut HashBuilder,
    updates: &'a mut TrieUpdates,
    stats: &'a mut StorageRootPrefetchStats,
    hashed_entries_walked: &'a mut usize,
    config: StorageRootPrefetchConfig,
}

fn process_account_prefix_barrier<H>(
    hashed_cursor_factory: H,
    barrier: AccountPrefixBarrier,
    has_more_after_barrier: bool,
    execution: &mut AccountPrefixExecution<'_>,
) -> Result<Option<StateRootProgress>, ParallelRebuildStateRootError>
where
    H: HashedCursorFactory + Clone + Send + Sync,
{
    let progress = complete_account_prefix_barrier_storage(
        hashed_cursor_factory,
        barrier.hashed_address,
        barrier.account,
        None,
        execution,
    )?;
    if progress.is_some() {
        return Ok(progress)
    }

    execution.stats.account_prefix_completed_barriers += 1;
    if has_more_after_barrier &&
        account_prefix_progress_threshold_reached(
            execution.updates,
            execution.hash_builder,
            execution.config,
        )
    {
        execution.stats.account_prefix_boundary_checkpoints += 1;
        return Ok(Some(create_progress(
            std::mem::take(execution.hash_builder),
            barrier.hashed_address,
            *execution.hashed_entries_walked,
            std::mem::take(execution.updates),
            None,
        )))
    }

    Ok(None)
}

fn process_account_prefix_single<H>(
    hashed_cursor_factory: H,
    single: AccountPrefixSingle,
    has_more_after_single: bool,
    execution: &mut AccountPrefixExecution<'_>,
) -> Result<Option<StateRootProgress>, ParallelRebuildStateRootError>
where
    H: HashedCursorFactory + Clone + Send + Sync,
{
    let account = {
        let mut account_cursor = hashed_cursor_factory.hashed_account_cursor()?;
        let Some((hashed_address, account)) = account_cursor.seek(single.hashed_address)? else {
            return Ok(None)
        };
        if hashed_address != single.hashed_address {
            return Ok(None)
        }
        account
    };

    let progress = complete_account_prefix_barrier_storage(
        hashed_cursor_factory,
        single.hashed_address,
        account,
        None,
        execution,
    )?;
    if progress.is_some() {
        return Ok(progress)
    }

    if has_more_after_single &&
        account_prefix_progress_threshold_reached(
            execution.updates,
            execution.hash_builder,
            execution.config,
        )
    {
        execution.stats.account_prefix_boundary_checkpoints += 1;
        return Ok(Some(create_progress(
            std::mem::take(execution.hash_builder),
            single.hashed_address,
            *execution.hashed_entries_walked,
            std::mem::take(execution.updates),
            None,
        )))
    }

    Ok(None)
}

fn complete_account_prefix_barrier_storage<H>(
    hashed_cursor_factory: H,
    hashed_address: B256,
    account: Account,
    state: Option<IntermediateRootState>,
    execution: &mut AccountPrefixExecution<'_>,
) -> Result<Option<StateRootProgress>, ParallelRebuildStateRootError>
where
    H: HashedCursorFactory + Clone + Send + Sync,
{
    let resuming = state.is_some();
    let threshold = account_prefix_storage_threshold(
        execution.updates,
        execution.hash_builder,
        execution.config,
    );
    let storage_job = calculate_storage_root_job(
        hashed_cursor_factory,
        hashed_address,
        threshold,
        state,
        execution.config,
        FreshStorageSegmentation::Auto,
    )?;
    execution.stats.extend(storage_job.stats);

    match storage_job.progress {
        StorageRootProgress::Complete(storage_root, walked, storage_updates) => {
            *execution.hashed_entries_walked += walked + usize::from(!resuming);
            let insert_started = std::time::Instant::now();
            execution.updates.insert_storage_updates(hashed_address, storage_updates);
            execution.stats.storage_updates_insert_duration += insert_started.elapsed();

            let account_leaf_started = std::time::Instant::now();
            let mut account_rlp = Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE);
            account.into_trie_account(storage_root).encode(&mut account_rlp as &mut dyn BufMut);
            execution.hash_builder.add_leaf(Nibbles::unpack(hashed_address), &account_rlp);
            execution.stats.account_leaf_duration += account_leaf_started.elapsed();

            Ok(None)
        }
        StorageRootProgress::Progress(state, walked, storage_updates) => {
            *execution.hashed_entries_walked += walked + usize::from(!resuming);
            let insert_started = std::time::Instant::now();
            execution.updates.insert_storage_updates(hashed_address, storage_updates);
            execution.stats.storage_updates_insert_duration += insert_started.elapsed();
            execution.stats.storage_progresses += 1;
            execution.stats.account_prefix_storage_progresses += 1;
            if resuming {
                execution.stats.resumed_storage_progresses += 1;
            }

            Ok(Some(create_progress(
                std::mem::take(execution.hash_builder),
                hashed_address,
                *execution.hashed_entries_walked,
                std::mem::take(execution.updates),
                Some(IntermediateStorageRootState { state: *state, account }),
            )))
        }
    }
}

fn account_prefix_storage_threshold(
    updates: &TrieUpdates,
    hash_builder: &HashBuilder,
    config: StorageRootPrefetchConfig,
) -> u64 {
    let total_updates_len = account_prefix_updates_len(updates, hash_builder) as u64;
    config.progress_threshold.saturating_sub(total_updates_len).max(1)
}

fn account_prefix_progress_threshold_reached(
    updates: &TrieUpdates,
    hash_builder: &HashBuilder,
    config: StorageRootPrefetchConfig,
) -> bool {
    account_prefix_updates_len(updates, hash_builder) as u64 >= config.progress_threshold
}

fn account_prefix_updates_len(updates: &TrieUpdates, hash_builder: &HashBuilder) -> usize {
    updates.account_nodes.len() +
        updates.removed_nodes.len() +
        updates.storage_tries.values().map(StorageTrieUpdates::len).sum::<usize>() +
        hash_builder.updates_len()
}

#[derive(Debug)]
enum AccountPrefixOutcome {
    Complete(Box<AccountPrefixResult>),
    Empty,
    InlineRoot(Nibbles),
    UnsupportedPrefix(Nibbles),
}

fn calculate_account_prefix_root<H>(
    hashed_cursor_factory: H,
    range: AccountPrefixRange,
    config: StorageRootPrefetchConfig,
) -> Result<AccountPrefixOutcome, ParallelRebuildStateRootError>
where
    H: HashedCursorFactory + Clone + Send + Sync,
{
    if range.prefix.is_empty() || range.prefix.len() >= 64 {
        return Ok(AccountPrefixOutcome::UnsupportedPrefix(range.prefix))
    }

    let storage_hints = range.storage_hints.clone();
    let mut storage_presence = if storage_hints.is_none() &&
        config.storage_prefix_planner.is_some() &&
        storage_prefix_gate_enabled(config)
    {
        hashed_cursor_factory
            .hashed_storage_key_cursor()?
            .map(|cursor| StoragePresenceCursor::from_start(cursor, range.start))
            .transpose()?
    } else {
        None
    };
    let mut build = AccountPrefixBuildState {
        hash_builder: HashBuilder::default().with_updates(true),
        updates: TrieUpdates::default(),
        stats: StorageRootPrefetchStats::default(),
        account_rlp: Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE),
        walked_accounts: 0,
        walked_storage_slots: 0,
    };
    let mut storage_hint_index = 0usize;

    let mut account_cursor = hashed_cursor_factory.hashed_account_cursor()?;
    let mut entry = account_cursor.seek(range.start)?;
    while let Some((hashed_address, account)) = entry {
        if range.end.is_some_and(|end| hashed_address >= end) {
            break;
        }

        let path = Nibbles::unpack(hashed_address);
        if !path.starts_with(&range.prefix) {
            break;
        }

        let storage_probe = account_prefix_storage_probe(
            &hashed_cursor_factory,
            storage_hints.as_deref(),
            &mut storage_hint_index,
            &mut storage_presence,
            hashed_address,
            config,
            &mut build.stats,
        )?;
        let storage_job = account_prefix_storage_job(
            &hashed_cursor_factory,
            hashed_address,
            storage_probe,
            config,
        )?;
        if !account_prefix_apply_account(
            &range.prefix,
            hashed_address,
            account,
            storage_job,
            &mut build,
        )? {
            return Ok(AccountPrefixOutcome::UnsupportedPrefix(range.prefix))
        }

        entry = account_cursor.next()?;
    }

    if build.walked_accounts == 0 {
        return Ok(AccountPrefixOutcome::Empty)
    }

    let _root = build.hash_builder.root();
    let Some(root_hash) = build.hash_builder.stack.last().and_then(|node| node.as_hash()) else {
        return Ok(AccountPrefixOutcome::InlineRoot(range.prefix))
    };
    let (_, raw_updates) = build.hash_builder.split();
    build.updates.account_nodes.extend(raw_updates.into_iter().map(|(relative_path, mut node)| {
        if relative_path.is_empty() {
            node.root_hash = None;
        }
        (range.prefix.join(&relative_path), node)
    }));

    Ok(AccountPrefixOutcome::Complete(Box::new(AccountPrefixResult {
        prefix: range.prefix,
        root_hash,
        last_hashed_address: account_prefix_last_key(&range.prefix),
        walked_accounts: build.walked_accounts,
        walked_storage_slots: build.walked_storage_slots,
        updates: build.updates,
        stats: build.stats,
    })))
}

fn account_prefix_storage_job<H>(
    hashed_cursor_factory: &H,
    hashed_address: B256,
    storage_probe: FreshStorageProbe,
    config: StorageRootPrefetchConfig,
) -> Result<StorageRootJobOutcome, ParallelRebuildStateRootError>
where
    H: HashedCursorFactory + Clone + Send + Sync,
{
    match storage_probe {
        FreshStorageProbe::Inline(inline_job) => Ok(inline_job.job),
        FreshStorageProbe::Miss(segmentation) => Ok(calculate_storage_root_job(
            hashed_cursor_factory.clone(),
            hashed_address,
            u64::MAX,
            None,
            config,
            segmentation.unwrap_or(FreshStorageSegmentation::Auto),
        )?),
    }
}

fn account_prefix_apply_account(
    prefix: &Nibbles,
    hashed_address: B256,
    account: Account,
    storage_job: StorageRootJobOutcome,
    build: &mut AccountPrefixBuildState,
) -> Result<bool, ParallelRebuildStateRootError> {
    let path = Nibbles::unpack(hashed_address);
    if !path.starts_with(prefix) {
        return Ok(false)
    }
    let relative_path = path.slice(prefix.len()..);
    if relative_path.is_empty() {
        return Ok(false)
    }

    build.stats.extend(storage_job.stats);
    let StorageRootProgress::Complete(storage_root, walked, storage_updates) = storage_job.progress
    else {
        return Ok(false)
    };

    build.walked_accounts += 1;
    build.walked_storage_slots += walked;

    let insert_started = std::time::Instant::now();
    build.updates.insert_storage_updates(hashed_address, storage_updates);
    build.stats.storage_updates_insert_duration += insert_started.elapsed();

    let account_leaf_started = std::time::Instant::now();
    build.account_rlp.clear();
    account.into_trie_account(storage_root).encode(&mut build.account_rlp as &mut dyn BufMut);
    build.hash_builder.add_leaf(relative_path, &build.account_rlp);
    build.stats.account_leaf_duration += account_leaf_started.elapsed();

    Ok(true)
}

fn account_prefix_storage_probe<H>(
    hashed_cursor_factory: &H,
    storage_hints: Option<&[AccountPrefixStorageHint]>,
    storage_hint_index: &mut usize,
    storage_presence: &mut Option<StoragePresenceCursor<'_>>,
    hashed_address: B256,
    config: StorageRootPrefetchConfig,
    stats: &mut StorageRootPrefetchStats,
) -> Result<FreshStorageProbe, ParallelRebuildStateRootError>
where
    H: HashedCursorFactory,
{
    let probe = if let Some(storage_hints) = storage_hints {
        account_prefix_storage_hint_probe(
            storage_hints,
            storage_hint_index,
            hashed_address,
            config,
            stats,
        )
    } else {
        account_prefix_storage_presence_probe(storage_presence, hashed_address, config, stats)?
    };
    account_prefix_finish_storage_probe(hashed_cursor_factory, hashed_address, probe, config, stats)
}

fn account_prefix_finish_storage_probe<H>(
    hashed_cursor_factory: &H,
    hashed_address: B256,
    probe: FreshStorageProbe,
    config: StorageRootPrefetchConfig,
    stats: &mut StorageRootPrefetchStats,
) -> Result<FreshStorageProbe, ParallelRebuildStateRootError>
where
    H: HashedCursorFactory,
{
    match probe {
        FreshStorageProbe::Miss(Some(FreshStorageSegmentation::Auto)) => {
            account_prefix_inline_small_storage_probe(
                hashed_cursor_factory,
                hashed_address,
                config,
                stats,
            )
        }
        probe => Ok(probe),
    }
}

fn account_prefix_storage_hint_probe(
    storage_hints: &[AccountPrefixStorageHint],
    storage_hint_index: &mut usize,
    hashed_address: B256,
    config: StorageRootPrefetchConfig,
    stats: &mut StorageRootPrefetchStats,
) -> FreshStorageProbe {
    let started = std::time::Instant::now();
    while storage_hints
        .get(*storage_hint_index)
        .is_some_and(|hint| hint.hashed_address < hashed_address)
    {
        *storage_hint_index += 1;
        stats.storage_presence_skipped_keys += 1;
    }
    stats.storage_presence_duration += started.elapsed();
    stats.storage_presence_checks += 1;

    if storage_hints
        .get(*storage_hint_index)
        .is_some_and(|hint| hint.hashed_address == hashed_address)
    {
        let hint = storage_hints[*storage_hint_index];
        *storage_hint_index += 1;
        stats.storage_presence_present_hits += 1;
        return account_prefix_probe_from_storage_presence(
            AccountStoragePresence::Present { slots: hint.slots },
            config,
            stats,
            false,
        )
    }

    stats.storage_presence_empty_hits += 1;
    account_prefix_probe_from_storage_presence(AccountStoragePresence::Empty, config, stats, false)
}

fn account_prefix_storage_presence_probe(
    storage_presence: &mut Option<StoragePresenceCursor<'_>>,
    hashed_address: B256,
    config: StorageRootPrefetchConfig,
    stats: &mut StorageRootPrefetchStats,
) -> Result<FreshStorageProbe, ParallelRebuildStateRootError> {
    let Some(storage_presence) = storage_presence.as_mut() else {
        return Ok(FreshStorageProbe::Miss(None))
    };

    let started = std::time::Instant::now();
    let (presence, skipped_keys) = storage_presence.lookup(hashed_address)?;
    stats.storage_presence_duration += started.elapsed();
    stats.storage_presence_checks += 1;
    stats.storage_presence_skipped_keys += skipped_keys;

    match presence {
        AccountStoragePresence::Empty => stats.storage_presence_empty_hits += 1,
        AccountStoragePresence::Present { .. } => stats.storage_presence_present_hits += 1,
    }

    Ok(account_prefix_probe_from_storage_presence(presence, config, stats, true))
}

fn account_prefix_probe_from_storage_presence(
    presence: AccountStoragePresence,
    config: StorageRootPrefetchConfig,
    stats: &mut StorageRootPrefetchStats,
    count_query: bool,
) -> FreshStorageProbe {
    match presence {
        AccountStoragePresence::Empty => {
            if config.inline_empty_storage_roots {
                stats.inline_empty_storage_roots += 1;
                return FreshStorageProbe::Inline(Box::new(inline_empty_storage_root_job()))
            }
            FreshStorageProbe::Miss(Some(account_prefix_serial_segmentation(config)))
        }
        AccountStoragePresence::Present { slots: Some(slots) } => {
            if slots > 0 &&
                config.inline_storage_root_slot_limit > 0 &&
                slots <= config.inline_storage_root_slot_limit
            {
                return FreshStorageProbe::Miss(Some(FreshStorageSegmentation::Auto))
            }
            FreshStorageProbe::Miss(Some(account_prefix_known_fresh_storage_segmentation(
                slots,
                config,
                stats,
                count_query,
            )))
        }
        AccountStoragePresence::Present { slots: None } => FreshStorageProbe::Miss(None),
    }
}

fn account_prefix_inline_small_storage_probe<H>(
    hashed_cursor_factory: &H,
    hashed_address: B256,
    config: StorageRootPrefetchConfig,
    stats: &mut StorageRootPrefetchStats,
) -> Result<FreshStorageProbe, ParallelRebuildStateRootError>
where
    H: HashedCursorFactory,
{
    let started = std::time::Instant::now();
    let Some(inline_job) = inline_storage_root_with_limit(
        hashed_cursor_factory,
        hashed_address,
        config.inline_storage_root_slot_limit,
    )?
    else {
        stats.inline_storage_root_check_duration += started.elapsed();
        stats.inline_storage_root_checks += 1;
        return Ok(FreshStorageProbe::Miss(Some(account_prefix_serial_segmentation(config))))
    };
    stats.inline_storage_root_check_duration += started.elapsed();
    stats.inline_storage_root_checks += 1;
    stats.inline_small_storage_roots += 1;
    stats.inline_small_storage_slots += inline_job.walked;
    Ok(FreshStorageProbe::Inline(Box::new(inline_job)))
}

fn inline_storage_root_with_limit<H>(
    hashed_cursor_factory: &H,
    hashed_address: B256,
    slot_limit: usize,
) -> Result<Option<InlineStorageRootJob>, StorageRootError>
where
    H: HashedCursorFactory,
{
    let mut cursor = hashed_cursor_factory.hashed_storage_cursor(hashed_address)?;
    let mut slots = Vec::new();
    let mut entry = cursor.seek(B256::ZERO)?;

    while let Some((hashed_slot, value)) = entry {
        if slots.len() >= slot_limit {
            return Ok(None)
        }
        slots.push((hashed_slot, value));
        entry = cursor.next()?;
    }

    Ok(Some(inline_storage_root_from_slots(slots)))
}

const fn account_prefix_serial_segmentation(
    config: StorageRootPrefetchConfig,
) -> FreshStorageSegmentation {
    if config.storage_prefix_planner.is_some() && storage_prefix_gate_enabled(config) {
        FreshStorageSegmentation::Serial
    } else {
        FreshStorageSegmentation::Auto
    }
}

fn account_prefix_known_fresh_storage_segmentation(
    slots: usize,
    config: StorageRootPrefetchConfig,
    stats: &mut StorageRootPrefetchStats,
    count_query: bool,
) -> FreshStorageSegmentation {
    if config.storage_prefix_planner.is_none() || !storage_prefix_gate_enabled(config) {
        return FreshStorageSegmentation::Auto
    }

    let started = std::time::Instant::now();
    stats.segmented_storage_gate_probes += 1;
    if count_query {
        stats.segmented_storage_gate_count_queries += 1;
        stats.segmented_storage_gate_counted_slots += slots;
    }

    let min_large_slots = config.storage_prefix_min_large_slots.max(1);
    let segmentation = if slots >= min_large_slots {
        stats.segmented_storage_gate_hits += 1;
        let planner_config = storage_prefix_adaptive_planner_config(
            config.storage_prefix_planner.expect("planner checked above"),
            config.storage_prefix_min_large_slots,
            Some(slots),
        );
        FreshStorageSegmentation::Segmented(planner_config)
    } else {
        stats.segmented_storage_gate_misses += 1;
        FreshStorageSegmentation::Serial
    };
    stats.segmented_storage_gate_duration += started.elapsed();
    segmentation
}

#[derive(Debug)]
enum FreshStorageProbe {
    Inline(Box<InlineStorageRootJob>),
    Miss(Option<FreshStorageSegmentation>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FreshStorageSegmentation {
    Auto,
    Segmented(StoragePrefixPlannerConfig),
    Serial,
}

impl FreshStorageSegmentation {
    const fn allows_segmented_trigger(&self, config: StorageRootPrefetchConfig) -> bool {
        matches!(self, Self::Auto) && !storage_prefix_gate_enabled(config)
    }
}
/// Error during experimental parallel clean rebuild calculation.
#[derive(Error, Debug)]
pub enum ParallelRebuildStateRootError {
    /// Error while calculating storage root.
    #[error(transparent)]
    StorageRoot(#[from] StorageRootError),
    /// Database error.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// Account-prefix rebuild was requested without enabling an account prefix depth.
    #[error("account-prefix rebuild requires account_prefix_max_depth > 0")]
    AccountPrefixRebuildDisabled,
    /// The checkpoint contains account trie walker state.
    #[error("parallel rebuild does not support account walker checkpoints yet")]
    UnsupportedAccountWalkerCheckpoint,
    /// Account prefix worker dropped its result.
    #[error("account prefix worker dropped result")]
    AccountPrefixWorkerDropped,
    /// The requested account prefix depth is not supported.
    #[error("invalid account prefix depth: {0}")]
    InvalidAccountPrefixDepth(usize),
    /// The account prefix cannot be merged as a closed subtree.
    #[error("unsupported account prefix: {0:?}")]
    UnsupportedAccountPrefix(Nibbles),
    /// The account prefix root was inline and cannot be merged with `add_branch`.
    #[error("inline account prefix root: {0:?}")]
    InlineAccountPrefixRoot(Nibbles),
}


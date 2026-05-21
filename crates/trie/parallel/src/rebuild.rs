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

fn calculate_storage_root_job<H>(
    hashed_cursor_factory: H,
    hashed_address: B256,
    threshold: u64,
    state: Option<IntermediateRootState>,
    config: StorageRootPrefetchConfig,
    fresh_storage_segmentation: FreshStorageSegmentation,
) -> Result<StorageRootJobOutcome, StorageRootError>
where
    H: HashedCursorFactory + Clone + Send + Sync,
{
    let started = std::time::Instant::now();
    let mut outcome = calculate_storage_root_job_inner(
        hashed_cursor_factory,
        hashed_address,
        threshold,
        state,
        config,
        fresh_storage_segmentation,
    )?;
    outcome.stats.storage_root_job_duration += started.elapsed();
    Ok(outcome)
}

fn calculate_storage_root_job_inner<H>(
    hashed_cursor_factory: H,
    hashed_address: B256,
    threshold: u64,
    state: Option<IntermediateRootState>,
    config: StorageRootPrefetchConfig,
    fresh_storage_segmentation: FreshStorageSegmentation,
) -> Result<StorageRootJobOutcome, StorageRootError>
where
    H: HashedCursorFactory + Clone + Send + Sync,
{
    let mut stats = StorageRootPrefetchStats::default();
    let fresh_storage = state.is_none();
    if let (Some(planner_config), Some(state)) = (config.storage_prefix_planner, state.as_ref()) {
        match calculate_segmented_storage_root(
            hashed_cursor_factory.clone(),
            hashed_address,
            threshold,
            Some(state),
            planner_config,
            &mut stats,
        ) {
            Ok(Some(progress)) => return Ok(StorageRootJobOutcome { progress, stats }),
            Ok(None) => stats.segmented_storage_fallbacks += 1,
            Err(err) => return Err(err),
        }
    }

    let planner_config = config.storage_prefix_planner;
    let fresh_storage_planner_config = if fresh_storage && planner_config.is_some() {
        match fresh_storage_segmentation {
            FreshStorageSegmentation::Segmented(planner_config) => Some(planner_config),
            FreshStorageSegmentation::Serial => None,
            FreshStorageSegmentation::Auto => {
                if storage_prefix_gate_enabled(config) {
                    let gate = storage_prefix_large_gate(
                        &hashed_cursor_factory,
                        hashed_address,
                        config.storage_prefix_min_large_slots,
                        &mut stats,
                    )?;
                    gate.large.then(|| {
                        storage_prefix_adaptive_planner_config(
                            planner_config.expect("planner checked above"),
                            config.storage_prefix_min_large_slots,
                            gate.slots,
                        )
                    })
                } else {
                    None
                }
            }
        }
    } else {
        None
    };
    if let (true, Some(planner_config)) = (fresh_storage, fresh_storage_planner_config) {
        let mut segmented_stats = StorageRootPrefetchStats::default();
        match calculate_segmented_storage_root(
            hashed_cursor_factory.clone(),
            hashed_address,
            threshold,
            None,
            planner_config,
            &mut segmented_stats,
        ) {
            Ok(Some(segmented_progress)) => {
                stats.extend(segmented_stats);
                return Ok(StorageRootJobOutcome { progress: segmented_progress, stats })
            }
            Ok(None) => {
                stats.extend(segmented_stats);
                stats.segmented_storage_fallbacks += 1;
            }
            Err(err) => return Err(err),
        }
    }

    let allow_segmented_trigger = fresh_storage &&
        planner_config.is_some() &&
        fresh_storage_segmentation.allows_segmented_trigger(config);
    let serial_threshold = if allow_segmented_trigger {
        threshold.min(config.storage_prefix_trigger_threshold.max(1))
    } else {
        threshold
    };
    let serial_started = std::time::Instant::now();
    let progress = calculate_storage_root(
        hashed_cursor_factory.clone(),
        hashed_address,
        serial_threshold,
        state,
    )?;
    stats.serial_storage_root_duration += serial_started.elapsed();
    if let (true, Some(planner_config), true, StorageRootProgress::Progress(..)) =
        (fresh_storage, planner_config, allow_segmented_trigger, &progress)
    {
        if serial_threshold >= threshold {
            return Ok(StorageRootJobOutcome { progress, stats })
        }

        let StorageRootProgress::Progress(_, walked, updates) = &progress else { unreachable!() };
        stats.segmented_storage_trigger_progresses += 1;
        stats.segmented_storage_trigger_discarded_slots += *walked;
        stats.segmented_storage_trigger_discarded_updates += updates.len();

        let mut segmented_stats = StorageRootPrefetchStats::default();
        match calculate_segmented_storage_root(
            hashed_cursor_factory,
            hashed_address,
            threshold,
            None,
            planner_config,
            &mut segmented_stats,
        ) {
            Ok(Some(segmented_progress)) => {
                stats.extend(segmented_stats);
                return Ok(StorageRootJobOutcome { progress: segmented_progress, stats })
            }
            Ok(None) => {
                stats.extend(segmented_stats);
                stats.segmented_storage_fallbacks += 1;
            }
            Err(err) => return Err(err),
        }
    }

    Ok(StorageRootJobOutcome { progress, stats })
}

const fn storage_prefix_gate_enabled(config: StorageRootPrefetchConfig) -> bool {
    config.storage_prefix_min_large_slots != usize::MAX
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StoragePrefixLargeGate {
    large: bool,
    slots: Option<usize>,
}

fn storage_prefix_large_gate<H>(
    hashed_cursor_factory: &H,
    hashed_address: B256,
    min_large_slots: usize,
    stats: &mut StorageRootPrefetchStats,
) -> Result<StoragePrefixLargeGate, StorageRootError>
where
    H: HashedCursorFactory,
{
    let started = std::time::Instant::now();
    stats.segmented_storage_gate_probes += 1;
    if let Some(mut cursor) = hashed_cursor_factory.hashed_storage_key_cursor()? {
        let slots = if cursor.seek_storage_key(hashed_address)? == Some(hashed_address) {
            cursor.current_storage_entry_count()?
        } else {
            Some(0)
        };

        if let Some(slots) = slots {
            return Ok(storage_prefix_large_gate_from_count(slots, min_large_slots, stats, started))
        }
    }

    let mut cursor = hashed_cursor_factory.hashed_storage_cursor(hashed_address)?;
    let mut slots = 0usize;
    let mut entry = cursor.seek(B256::ZERO)?;

    while let Some((_hashed_slot, _value)) = entry {
        slots += 1;
        if slots >= min_large_slots.max(1) {
            stats.segmented_storage_gate_slots += slots;
            stats.segmented_storage_gate_hits += 1;
            stats.segmented_storage_gate_duration += started.elapsed();
            return Ok(StoragePrefixLargeGate { large: true, slots: Some(slots) })
        }
        entry = cursor.next()?;
    }

    stats.segmented_storage_gate_slots += slots;
    stats.segmented_storage_gate_misses += 1;
    stats.segmented_storage_gate_duration += started.elapsed();
    Ok(StoragePrefixLargeGate { large: false, slots: Some(slots) })
}

fn storage_prefix_large_gate_from_count(
    slots: usize,
    min_large_slots: usize,
    stats: &mut StorageRootPrefetchStats,
    started: std::time::Instant,
) -> StoragePrefixLargeGate {
    stats.segmented_storage_gate_count_queries += 1;
    stats.segmented_storage_gate_counted_slots += slots;
    if slots >= min_large_slots.max(1) {
        stats.segmented_storage_gate_hits += 1;
        stats.segmented_storage_gate_duration += started.elapsed();
        return StoragePrefixLargeGate { large: true, slots: Some(slots) }
    }

    stats.segmented_storage_gate_misses += 1;
    stats.segmented_storage_gate_duration += started.elapsed();
    StoragePrefixLargeGate { large: false, slots: Some(slots) }
}

fn storage_prefix_adaptive_planner_config(
    mut config: StoragePrefixPlannerConfig,
    min_large_slots: usize,
    slots: Option<usize>,
) -> StoragePrefixPlannerConfig {
    let Some(slots) = slots else { return config };
    let min_large_slots = min_large_slots.max(1);
    let depth_cap = if slots >= min_large_slots.saturating_mul(256) {
        4
    } else if slots >= min_large_slots.saturating_mul(16) {
        3
    } else {
        2
    };
    config.max_depth = config.max_depth.min(depth_cap);
    config
}

fn calculate_segmented_storage_root<H>(
    hashed_cursor_factory: H,
    hashed_address: B256,
    threshold: u64,
    state: Option<&IntermediateRootState>,
    planner_config: StoragePrefixPlannerConfig,
    stats: &mut StorageRootPrefetchStats,
) -> Result<Option<StorageRootProgress>, StorageRootError>
where
    H: HashedCursorFactory + Clone + Send + Sync,
{
    let started = std::time::Instant::now();
    if state.is_some_and(|state| !is_potential_segmented_storage_checkpoint(state)) {
        stats.segmented_storage_total_duration += started.elapsed();
        return Ok(None)
    }

    stats.segmented_storage_attempts += 1;

    let Some(result) = build_segmented_storage_root(
        hashed_cursor_factory,
        hashed_address,
        planner_config,
        threshold,
        state,
        stats,
    )?
    else {
        stats.segmented_storage_total_duration += started.elapsed();
        return Ok(None)
    };

    if result.used_serial_fallback {
        stats.segmented_storage_fallbacks += 1;
    }
    if result.completed_by_segmented {
        stats.segmented_storage_roots += 1;
    }
    stats.segmented_storage_prefixes += result.segmented_prefixes;
    stats.segmented_storage_slots += result.segmented_walked;
    stats.segmented_storage_total_duration += started.elapsed();

    Ok(Some(result.progress))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegmentedStoragePlanUnsupportedReason {
    SingleRange,
    UnsupportedPrefix,
}

fn unsupported_segmented_storage_plan_reason(
    plan: &StoragePrefixPlan,
) -> Option<SegmentedStoragePlanUnsupportedReason> {
    if plan.ranges.is_empty() && !plan.complete {
        return Some(SegmentedStoragePlanUnsupportedReason::SingleRange)
    }
    if plan.ranges.iter().any(|range| range.prefix.is_empty() || range.prefix.len() >= 64) {
        return Some(SegmentedStoragePlanUnsupportedReason::UnsupportedPrefix)
    }

    None
}

fn build_segmented_storage_root<H>(
    hashed_cursor_factory: H,
    hashed_address: B256,
    planner_config: StoragePrefixPlannerConfig,
    threshold: u64,
    state: Option<&IntermediateRootState>,
    stats: &mut StorageRootPrefetchStats,
) -> Result<Option<SegmentedStorageBuildOutcome>, StorageRootError>
where
    H: HashedCursorFactory + Clone + Send + Sync,
{
    let resume_started = std::time::Instant::now();
    let Some(resume) = segmented_storage_resume(&hashed_cursor_factory, hashed_address, state)?
    else {
        stats.segmented_storage_resume_duration += resume_started.elapsed();
        stats.segmented_storage_resume_fallbacks += 1;
        return Ok(None)
    };
    stats.segmented_storage_resume_duration += resume_started.elapsed();

    if let Some(completed_prefix) = resume.cursor.completed_prefix {
        debug_assert_eq!(resume.cursor.next_start_bound, storage_prefix_end(&completed_prefix));
    }

    let mut parent_hash_builder = resume.hash_builder;
    let mut updates = StorageTrieUpdates::default();
    let mut walked = 0usize;
    let mut segmented_prefixes = 0usize;
    let mut last_hashed_slot = resume.cursor.last_segment_slot;
    let mut next_start_bound = resume.cursor.next_start_bound;
    let wave_size = segmented_storage_wave_size();
    let mut update_estimator = SegmentedStorageUpdateEstimator::default();

    loop {
        let plan_budget = segmented_storage_plan_budget(threshold, &update_estimator)
            .map(|budget| StoragePrefixPlanBudget::new(update_estimator, budget))
            .unwrap_or_else(StoragePrefixPlanBudget::unbounded);
        let planning_started = std::time::Instant::now();
        let mut plan = plan_storage_prefixes_after(
            &hashed_cursor_factory,
            hashed_address,
            next_start_bound,
            planner_config,
            plan_budget,
        )?;
        stats.segmented_storage_planning_duration += planning_started.elapsed();
        stats.record_storage_prefix_plan(&plan.stats);
        if !plan.complete {
            stats.segmented_storage_partial_plans += 1;
            if plan.stats.too_many_prefixes == 0 {
                stats.segmented_storage_budget_stops += 1;
            }
        }

        if let Some(reason) = unsupported_segmented_storage_plan_reason(&plan) {
            match reason {
                SegmentedStoragePlanUnsupportedReason::SingleRange => {
                    stats.segmented_storage_plan_single_range_fallbacks += 1;
                }
                SegmentedStoragePlanUnsupportedReason::UnsupportedPrefix => {
                    stats.segmented_storage_plan_unsupported_prefix_fallbacks += 1;
                }
            }
            return Ok(None)
        }

        plan.ranges.sort_unstable_by_key(|range| range.start);
        if let Some(last_segment_slot) = last_hashed_slot &&
            !segmented_storage_resume_matches_next_range(
                &hashed_cursor_factory,
                hashed_address,
                plan.ranges.first(),
                last_segment_slot,
            )?
        {
            stats.segmented_storage_resume_fallbacks += 1;
            return Ok(None)
        }

        if plan.ranges.is_empty() && plan.complete {
            let root = parent_hash_builder.root();
            extend_with_parent_updates(&mut updates, &mut parent_hash_builder, false);

            return Ok(Some(SegmentedStorageBuildOutcome {
                progress: StorageRootProgress::Complete(root, walked, updates),
                segmented_prefixes,
                segmented_walked: walked,
                completed_by_segmented: true,
                used_serial_fallback: false,
            }))
        }

        let plan_complete = plan.complete;
        let ranges = plan.ranges;
        let mut next_range_index = 0usize;
        while next_range_index < ranges.len() {
            let wave_end = segmented_storage_wave_end(SegmentedStorageWaveEnd {
                ranges: &ranges,
                next_range_index,
                wave_size,
                updates: &updates,
                hash_builder: &parent_hash_builder,
                threshold,
                update_estimator: &update_estimator,
                stats,
            });
            let wave_len = wave_end - next_range_index;
            let (tx, rx) = std::sync::mpsc::sync_channel(wave_len);

            let wave_started = std::time::Instant::now();
            rayon::scope(|scope| {
                for (offset, range) in
                    ranges[next_range_index..wave_end].iter().cloned().enumerate()
                {
                    let tx = tx.clone();
                    let hashed_cursor_factory = hashed_cursor_factory.clone();
                    scope.spawn(move |_| {
                        let worker_started = std::time::Instant::now();
                        let result = calculate_storage_prefix_root(
                            hashed_cursor_factory,
                            hashed_address,
                            range,
                        );
                        let _ = tx.send((offset, worker_started.elapsed(), result));
                    });
                }
            });
            stats.segmented_storage_wave_compute_duration += wave_started.elapsed();
            drop(tx);

            let mut prefix_results = std::iter::repeat_with(|| None)
                .take(wave_len)
                .collect::<Vec<Option<SegmentedStoragePrefixResult>>>();
            for _ in 0..wave_len {
                let Ok((offset, worker_elapsed, result)) = rx.recv() else {
                    stats.segmented_storage_missing_prefix_result_fallbacks += 1;
                    return finish_segmented_storage_with_serial_fallback(
                        hashed_cursor_factory,
                        hashed_address,
                        SegmentedStoragePartial {
                            parent_hash_builder,
                            last_hashed_slot,
                            walked,
                            updates,
                            segmented_prefixes,
                        },
                        stats,
                        threshold,
                    )
                };
                stats.segmented_storage_prefix_worker_duration += worker_elapsed;
                match result {
                    Ok(SegmentedStoragePrefixOutcome::Complete(result)) => {
                        prefix_results[offset] = Some(*result);
                    }
                    Ok(SegmentedStoragePrefixOutcome::InlineRoot) => {
                        stats.segmented_storage_inline_fallbacks += 1;
                        return finish_segmented_storage_with_serial_fallback(
                            hashed_cursor_factory,
                            hashed_address,
                            SegmentedStoragePartial {
                                parent_hash_builder,
                                last_hashed_slot,
                                walked,
                                updates,
                                segmented_prefixes,
                            },
                            stats,
                            threshold,
                        )
                    }
                    Ok(SegmentedStoragePrefixOutcome::UnsupportedRoot) => {
                        stats.segmented_storage_prefix_unsupported_root_fallbacks += 1;
                        return finish_segmented_storage_with_serial_fallback(
                            hashed_cursor_factory,
                            hashed_address,
                            SegmentedStoragePartial {
                                parent_hash_builder,
                                last_hashed_slot,
                                walked,
                                updates,
                                segmented_prefixes,
                            },
                            stats,
                            threshold,
                        )
                    }
                    Ok(SegmentedStoragePrefixOutcome::EmptyRange) => {
                        stats.segmented_storage_prefix_empty_range_fallbacks += 1;
                        return finish_segmented_storage_with_serial_fallback(
                            hashed_cursor_factory,
                            hashed_address,
                            SegmentedStoragePartial {
                                parent_hash_builder,
                                last_hashed_slot,
                                walked,
                                updates,
                                segmented_prefixes,
                            },
                            stats,
                            threshold,
                        )
                    }
                    Err(err) => return Err(err),
                }
            }

            let merge_started = std::time::Instant::now();
            for (offset, prefix_result) in prefix_results.into_iter().enumerate() {
                let Some(prefix_result) = prefix_result else {
                    stats.segmented_storage_missing_prefix_result_fallbacks += 1;
                    stats.segmented_storage_prefix_merge_duration += merge_started.elapsed();
                    return finish_segmented_storage_with_serial_fallback(
                        hashed_cursor_factory,
                        hashed_address,
                        SegmentedStoragePartial {
                            parent_hash_builder,
                            last_hashed_slot,
                            walked,
                            updates,
                            segmented_prefixes,
                        },
                        stats,
                        threshold,
                    )
                };
                let range_index = next_range_index + offset;
                let has_more_after_prefix = range_index + 1 < ranges.len() || !plan_complete;
                last_hashed_slot = Some(prefix_result.last_hashed_slot);
                next_start_bound = storage_prefix_end(&prefix_result.prefix);

                let children_are_in_trie = !prefix_result.updates.storage_nodes.is_empty();
                parent_hash_builder.add_branch(
                    prefix_result.prefix,
                    prefix_result.root_hash,
                    children_are_in_trie,
                );
                stats.segmented_storage_prefix_cached_slots += prefix_result.cached_slots;
                update_estimator.record(prefix_result.walked, prefix_result.updates.len());
                walked += prefix_result.walked;
                segmented_prefixes += 1;
                updates.extend(prefix_result.updates);

                if has_more_after_prefix &&
                    segmented_storage_threshold_reached(
                        &updates,
                        &parent_hash_builder,
                        threshold,
                    )
                {
                    stats.segmented_storage_prefix_merge_duration += merge_started.elapsed();
                    return finish_segmented_storage_with_progress(SegmentedStoragePartial {
                        parent_hash_builder,
                        last_hashed_slot,
                        walked,
                        updates,
                        segmented_prefixes,
                    })
                }
            }
            stats.segmented_storage_prefix_merge_duration += merge_started.elapsed();

            next_range_index = wave_end;
        }

        if plan_complete {
            let root = parent_hash_builder.root();
            extend_with_parent_updates(&mut updates, &mut parent_hash_builder, false);

            return Ok(Some(SegmentedStorageBuildOutcome {
                progress: StorageRootProgress::Complete(root, walked, updates),
                segmented_prefixes,
                segmented_walked: walked,
                completed_by_segmented: true,
                used_serial_fallback: false,
            }))
        }
    }
}

fn finish_segmented_storage_with_progress(
    partial: SegmentedStoragePartial,
) -> Result<Option<SegmentedStorageBuildOutcome>, StorageRootError> {
    let SegmentedStoragePartial {
        mut parent_hash_builder,
        last_hashed_slot,
        walked,
        mut updates,
        segmented_prefixes,
    } = partial;
    let Some(last_hashed_slot) = last_hashed_slot else { return Ok(None) };

    extend_with_parent_updates(&mut updates, &mut parent_hash_builder, true);
    let state = IntermediateRootState {
        hash_builder: parent_hash_builder,
        walker_stack: Vec::new(),
        last_hashed_key: last_hashed_slot,
    };

    Ok(Some(SegmentedStorageBuildOutcome {
        progress: StorageRootProgress::Progress(Box::new(state), walked, updates),
        segmented_prefixes,
        segmented_walked: walked,
        completed_by_segmented: false,
        used_serial_fallback: false,
    }))
}

fn finish_segmented_storage_with_serial_fallback<H>(
    hashed_cursor_factory: H,
    hashed_address: B256,
    partial: SegmentedStoragePartial,
    stats: &mut StorageRootPrefetchStats,
    threshold: u64,
) -> Result<Option<SegmentedStorageBuildOutcome>, StorageRootError>
where
    H: HashedCursorFactory,
{
    let SegmentedStoragePartial {
        mut parent_hash_builder,
        last_hashed_slot,
        walked,
        mut updates,
        segmented_prefixes,
    } = partial;
    let Some(last_hashed_slot) = last_hashed_slot else { return Ok(None) };

    extend_with_parent_updates(&mut updates, &mut parent_hash_builder, true);
    let remaining_threshold = threshold.saturating_sub(updates.len() as u64).max(1);
    let state = IntermediateRootState {
        hash_builder: parent_hash_builder,
        walker_stack: Vec::new(),
        last_hashed_key: last_hashed_slot,
    };
    let serial_started = std::time::Instant::now();
    let progress = calculate_storage_root(
        hashed_cursor_factory,
        hashed_address,
        remaining_threshold,
        Some(state),
    )?;
    let serial_elapsed = serial_started.elapsed();
    stats.segmented_storage_serial_fallback_duration += serial_elapsed;
    stats.serial_storage_root_duration += serial_elapsed;

    let progress = match progress {
        StorageRootProgress::Complete(root, serial_walked, serial_updates) => {
            updates.extend(serial_updates);
            StorageRootProgress::Complete(root, walked + serial_walked, updates)
        }
        StorageRootProgress::Progress(state, serial_walked, serial_updates) => {
            updates.extend(serial_updates);
            StorageRootProgress::Progress(state, walked + serial_walked, updates)
        }
    };

    Ok(Some(SegmentedStorageBuildOutcome {
        progress,
        segmented_prefixes,
        segmented_walked: walked,
        completed_by_segmented: false,
        used_serial_fallback: true,
    }))
}

fn segmented_storage_resume<H>(
    hashed_cursor_factory: &H,
    hashed_address: B256,
    state: Option<&IntermediateRootState>,
) -> Result<Option<SegmentedStorageResume>, DatabaseError>
where
    H: HashedCursorFactory,
{
    let Some(state) = state else {
        return Ok(Some(SegmentedStorageResume {
            hash_builder: HashBuilder::default().with_updates(true),
            cursor: SegmentedResumeCursor {
                completed_prefix: None,
                last_segment_slot: None,
                next_start_bound: Some(B256::ZERO),
            },
        }))
    };

    let Some(cursor) = segmented_resume_cursor(hashed_cursor_factory, hashed_address, state)?
    else {
        return Ok(None)
    };

    Ok(Some(SegmentedStorageResume {
        hash_builder: state.hash_builder.clone().with_updates(true),
        cursor,
    }))
}

fn segmented_resume_cursor<H>(
    hashed_cursor_factory: &H,
    hashed_address: B256,
    state: &IntermediateRootState,
) -> Result<Option<SegmentedResumeCursor>, DatabaseError>
where
    H: HashedCursorFactory,
{
    if !is_potential_segmented_storage_checkpoint(state) {
        return Ok(None)
    }

    let completed_prefix = state.hash_builder.key;
    let last_segment_slot = state.last_hashed_key;
    if !segmented_storage_completed_prefix_matches(
        hashed_cursor_factory,
        hashed_address,
        &completed_prefix,
        last_segment_slot,
    )? {
        return Ok(None)
    }

    Ok(Some(SegmentedResumeCursor {
        completed_prefix: Some(completed_prefix),
        last_segment_slot: Some(last_segment_slot),
        next_start_bound: storage_prefix_end(&completed_prefix),
    }))
}

const fn is_potential_segmented_storage_checkpoint(state: &IntermediateRootState) -> bool {
    state.walker_stack.is_empty() &&
        !state.hash_builder.key.is_empty() &&
        state.hash_builder.key.len() < 64
}

fn segmented_storage_completed_prefix_matches<H>(
    hashed_cursor_factory: &H,
    hashed_address: B256,
    completed_prefix: &Nibbles,
    last_hashed_slot: B256,
) -> Result<bool, DatabaseError>
where
    H: HashedCursorFactory,
{
    let completed_range = StoragePrefixRange {
        prefix: *completed_prefix,
        start: storage_prefix_bound(completed_prefix),
        end: storage_prefix_end(completed_prefix),
        sampled_slots: 0,
        sampled_entries: Vec::new(),
    };
    if !storage_slot_in_range(last_hashed_slot, &completed_range) {
        return Ok(false)
    }

    storage_range_last_slot_matches(
        hashed_cursor_factory,
        hashed_address,
        &completed_range,
        last_hashed_slot,
    )
}

fn storage_range_last_slot_matches<H>(
    hashed_cursor_factory: &H,
    hashed_address: B256,
    range: &StoragePrefixRange,
    last_hashed_slot: B256,
) -> Result<bool, DatabaseError>
where
    H: HashedCursorFactory,
{
    let mut cursor = hashed_cursor_factory.hashed_storage_cursor(hashed_address)?;
    let Some((found_slot, _)) = cursor.seek(last_hashed_slot)? else { return Ok(false) };
    if found_slot != last_hashed_slot {
        return Ok(false)
    }

    Ok(cursor.next()?.is_none_or(|(next_slot, _)| !storage_slot_in_range(next_slot, range)))
}

fn segmented_storage_resume_matches_next_range<H>(
    hashed_cursor_factory: &H,
    hashed_address: B256,
    next_range: Option<&StoragePrefixRange>,
    last_hashed_slot: B256,
) -> Result<bool, DatabaseError>
where
    H: HashedCursorFactory,
{
    let mut cursor = hashed_cursor_factory.hashed_storage_cursor(hashed_address)?;
    let Some((found_slot, _)) = cursor.seek(last_hashed_slot)? else { return Ok(false) };
    if found_slot != last_hashed_slot {
        return Ok(false)
    }

    let Some((next_slot, _)) = cursor.next()? else { return Ok(next_range.is_none()) };
    Ok(next_range.is_some_and(|range| storage_slot_in_range(next_slot, range)))
}

fn storage_slot_in_range(hashed_slot: B256, range: &StoragePrefixRange) -> bool {
    hashed_slot >= range.start &&
        range.end.is_none_or(|end| hashed_slot < end) &&
        Nibbles::unpack(hashed_slot).starts_with(&range.prefix)
}

fn segmented_storage_wave_size() -> usize {
    std::thread::available_parallelism().map_or(4, usize::from).clamp(1, 16)
}

fn segmented_storage_plan_budget(
    threshold: u64,
    update_estimator: &SegmentedStorageUpdateEstimator,
) -> Option<u64> {
    if threshold == u64::MAX {
        return None
    }

    Some((threshold.saturating_mul(4) / 5).max(update_estimator.minimum_budget()))
}

struct SegmentedStorageWaveEnd<'a> {
    ranges: &'a [StoragePrefixRange],
    next_range_index: usize,
    wave_size: usize,
    updates: &'a StorageTrieUpdates,
    hash_builder: &'a HashBuilder,
    threshold: u64,
    update_estimator: &'a SegmentedStorageUpdateEstimator,
    stats: &'a mut StorageRootPrefetchStats,
}

fn segmented_storage_wave_end(input: SegmentedStorageWaveEnd<'_>) -> usize {
    let SegmentedStorageWaveEnd {
        ranges,
        next_range_index,
        wave_size,
        updates,
        hash_builder,
        threshold,
        update_estimator,
        stats,
    } = input;
    let max_wave_end = (next_range_index + wave_size).min(ranges.len());
    if threshold == u64::MAX {
        return max_wave_end
    }

    let remaining = threshold.saturating_sub((updates.len() + hash_builder.updates_len()) as u64);
    let target_budget = remaining.saturating_mul(4) / 5;
    let mut estimated_updates = 0u64;
    let mut wave_end = next_range_index;
    while wave_end < max_wave_end {
        let estimate = update_estimator.estimate(&ranges[wave_end]);
        if wave_end > next_range_index && estimated_updates.saturating_add(estimate) > target_budget
        {
            stats.segmented_storage_budget_stops += 1;
            break;
        }
        estimated_updates = estimated_updates.saturating_add(estimate);
        wave_end += 1;
    }

    if wave_end == next_range_index {
        stats.segmented_storage_budget_stops += 1;
        (next_range_index + 1).min(ranges.len())
    } else {
        wave_end
    }
}

fn segmented_storage_threshold_reached(
    updates: &StorageTrieUpdates,
    hash_builder: &HashBuilder,
    threshold: u64,
) -> bool {
    threshold != u64::MAX && (updates.len() + hash_builder.updates_len()) as u64 >= threshold
}

fn extend_with_parent_updates(
    updates: &mut StorageTrieUpdates,
    hash_builder: &mut HashBuilder,
    include_empty: bool,
) {
    let (split_hash_builder, parent_updates) = std::mem::take(hash_builder).split();
    *hash_builder = split_hash_builder;
    if include_empty {
        updates.storage_nodes.extend(parent_updates);
    } else {
        updates
            .storage_nodes
            .extend(parent_updates.into_iter().filter(|(path, _)| !path.is_empty()));
    }
}

#[derive(Debug)]
struct SegmentedStorageResume {
    hash_builder: HashBuilder,
    cursor: SegmentedResumeCursor,
}

#[derive(Debug, Clone, Copy)]
struct SegmentedResumeCursor {
    completed_prefix: Option<Nibbles>,
    last_segment_slot: Option<B256>,
    next_start_bound: Option<B256>,
}

#[derive(Debug)]
struct SegmentedStoragePartial {
    parent_hash_builder: HashBuilder,
    last_hashed_slot: Option<B256>,
    walked: usize,
    updates: StorageTrieUpdates,
    segmented_prefixes: usize,
}

#[derive(Debug)]
struct SegmentedStorageBuildOutcome {
    progress: StorageRootProgress,
    segmented_prefixes: usize,
    segmented_walked: usize,
    completed_by_segmented: bool,
    used_serial_fallback: bool,
}

#[derive(Debug)]
enum SegmentedStoragePrefixOutcome {
    Complete(Box<SegmentedStoragePrefixResult>),
    InlineRoot,
    UnsupportedRoot,
    EmptyRange,
}

#[derive(Debug)]
struct SegmentedStoragePrefixResult {
    prefix: Nibbles,
    root_hash: B256,
    last_hashed_slot: B256,
    walked: usize,
    cached_slots: usize,
    updates: StorageTrieUpdates,
}

fn calculate_storage_prefix_root<H>(
    hashed_cursor_factory: H,
    hashed_address: B256,
    range: StoragePrefixRange,
) -> Result<SegmentedStoragePrefixOutcome, StorageRootError>
where
    H: HashedCursorFactory,
{
    let mut cursor = hashed_cursor_factory.hashed_storage_cursor(hashed_address)?;
    let mut hash_builder = HashBuilder::default().with_updates(true);
    let mut walked = 0usize;
    let mut cached_slots = 0usize;
    let mut last_hashed_slot = None;

    for (hashed_slot, value) in range.sampled_entries.iter().copied() {
        if !storage_slot_in_range(hashed_slot, &range) {
            break;
        }
        if add_storage_prefix_leaf(&mut hash_builder, &range, hashed_slot, value).is_err() {
            return Ok(SegmentedStoragePrefixOutcome::UnsupportedRoot)
        }
        walked += 1;
        cached_slots += 1;
        last_hashed_slot = Some(hashed_slot);
    }

    let mut entry = if let Some(last_hashed_slot) = last_hashed_slot {
        match cursor.seek(last_hashed_slot)? {
            Some((found_slot, _)) if found_slot == last_hashed_slot => cursor.next()?,
            _ => cursor.seek(range.start)?,
        }
    } else {
        cursor.seek(range.start)?
    };

    while let Some((hashed_slot, value)) = entry {
        if range.end.is_some_and(|end| hashed_slot >= end) {
            break;
        }

        let path = Nibbles::unpack(hashed_slot);
        if !path.starts_with(&range.prefix) {
            break;
        }

        if add_storage_prefix_leaf(&mut hash_builder, &range, hashed_slot, value).is_err() {
            return Ok(SegmentedStoragePrefixOutcome::UnsupportedRoot)
        }
        walked += 1;
        last_hashed_slot = Some(hashed_slot);
        entry = cursor.next()?;
    }

    if walked == 0 {
        return Ok(SegmentedStoragePrefixOutcome::EmptyRange)
    }

    let _root = hash_builder.root();
    let Some(root_hash) = hash_builder.stack.last().and_then(|node| node.as_hash()) else {
        return Ok(SegmentedStoragePrefixOutcome::InlineRoot)
    };
    let (_, raw_updates) = hash_builder.split();

    let mut updates = StorageTrieUpdates::default();
    updates.storage_nodes.extend(raw_updates.into_iter().map(|(relative_path, mut node)| {
        if relative_path.is_empty() {
            node.root_hash = None;
        }
        (range.prefix.join(&relative_path), node)
    }));

    Ok(SegmentedStoragePrefixOutcome::Complete(Box::new(SegmentedStoragePrefixResult {
        prefix: range.prefix,
        root_hash,
        last_hashed_slot: last_hashed_slot.expect("walked prefix range has a hashed slot"),
        walked,
        cached_slots,
        updates,
    })))
}

fn add_storage_prefix_leaf(
    hash_builder: &mut HashBuilder,
    range: &StoragePrefixRange,
    hashed_slot: B256,
    value: U256,
) -> Result<(), SegmentedStoragePrefixOutcome> {
    let path = Nibbles::unpack(hashed_slot);
    let relative_path = path.slice(range.prefix.len()..);
    if relative_path.is_empty() {
        return Err(SegmentedStoragePrefixOutcome::UnsupportedRoot)
    }

    hash_builder.add_leaf(relative_path, alloy_rlp::encode_fixed_size(&value).as_ref());
    Ok(())
}

fn calculate_storage_root<H>(
    hashed_cursor_factory: H,
    hashed_address: B256,
    threshold: u64,
    state: Option<IntermediateRootState>,
) -> Result<StorageRootProgress, StorageRootError>
where
    H: HashedCursorFactory,
{
    StorageRoot::new_hashed(
        NoopTrieCursorFactory::default(),
        hashed_cursor_factory,
        hashed_address,
        PrefixSet::default(),
        #[cfg(feature = "metrics")]
        Default::default(),
    )
    .with_intermediate_state(state)
    .with_threshold(threshold)
    .calculate(true)
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


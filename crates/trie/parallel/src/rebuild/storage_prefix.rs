use alloy_primitives::{B256, U256};
use reth_storage_errors::db::DatabaseError;
use reth_trie::{
    hashed_cursor::{HashedCursor, HashedCursorFactory},
    Nibbles,
};

/// Experimental storage prefix planner configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoragePrefixPlannerConfig {
    /// Maximum trie prefix depth considered by the planner.
    pub max_depth: usize,
    /// Number of slots sampled while deciding whether to split a prefix.
    pub sample_limit_per_prefix: usize,
    /// Maximum number of prefix ranges emitted by one plan.
    pub max_prefixes: usize,
    /// Minimum sampled slots required before a prefix is split.
    pub min_sampled_slots_to_split: usize,
}

impl Default for StoragePrefixPlannerConfig {
    fn default() -> Self {
        Self {
            max_depth: 2,
            sample_limit_per_prefix: 64,
            max_prefixes: 256,
            min_sampled_slots_to_split: 64,
        }
    }
}

impl StoragePrefixPlannerConfig {
    fn normalized(self) -> Self {
        Self {
            max_depth: self.max_depth.min(64),
            sample_limit_per_prefix: self.sample_limit_per_prefix.max(1),
            max_prefixes: self.max_prefixes.max(1),
            min_sampled_slots_to_split: self.min_sampled_slots_to_split.max(1),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct StoragePrefixPlan {
    pub ranges: Vec<StoragePrefixRange>,
    pub complete: bool,
    pub stats: StoragePrefixPlannerStats,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StoragePrefixRange {
    pub prefix: Nibbles,
    pub start: B256,
    pub end: Option<B256>,
    pub sampled_slots: usize,
    pub sampled_entries: Vec<(B256, U256)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct StoragePrefixPlannerStats {
    pub probes: usize,
    pub sampled_slots: usize,
    pub reusable_sampled_slots: usize,
    pub max_depth_observed: usize,
    pub split_prefixes: usize,
    pub empty_prefixes: usize,
    pub planned_prefixes: usize,
    pub too_many_prefixes: usize,
    pub too_deep_prefixes: usize,
}

#[cfg(test)]
pub(super) fn plan_storage_prefixes<H>(
    hashed_cursor_factory: &H,
    hashed_address: B256,
    config: StoragePrefixPlannerConfig,
) -> Result<StoragePrefixPlan, DatabaseError>
where
    H: HashedCursorFactory,
{
    plan_storage_prefixes_after(
        hashed_cursor_factory,
        hashed_address,
        Some(B256::ZERO),
        config,
        StoragePrefixPlanBudget::unbounded(),
    )
}

pub(super) fn plan_storage_prefixes_after<H>(
    hashed_cursor_factory: &H,
    hashed_address: B256,
    start_bound: Option<B256>,
    config: StoragePrefixPlannerConfig,
    budget: StoragePrefixPlanBudget,
) -> Result<StoragePrefixPlan, DatabaseError>
where
    H: HashedCursorFactory,
{
    let config = config.normalized();
    let mut cursor = hashed_cursor_factory.hashed_storage_cursor(hashed_address)?;
    let mut plan = StoragePrefixPlan::default();
    let Some(start_bound) = start_bound else {
        plan.complete = true;
        return Ok(plan)
    };
    let mut budget = budget;
    plan.complete = plan_storage_prefix(
        &mut cursor,
        Nibbles::new(),
        start_bound,
        config,
        &mut budget,
        &mut plan,
    )? == StoragePrefixPlanStatus::Complete;
    Ok(plan)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StoragePrefixPlanStatus {
    Complete,
    TooManyPrefixes,
    BudgetReached,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StoragePrefixProbe {
    sampled_slots: usize,
    sampled_entries: Vec<(B256, U256)>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct StoragePrefixPlanBudget {
    update_estimator: SegmentedStorageUpdateEstimator,
    estimated_update_budget: Option<u64>,
    estimated_updates: u64,
}

impl StoragePrefixPlanBudget {
    pub(super) const fn unbounded() -> Self {
        Self {
            update_estimator: SegmentedStorageUpdateEstimator::new(),
            estimated_update_budget: None,
            estimated_updates: 0,
        }
    }

    pub(super) const fn new(
        update_estimator: SegmentedStorageUpdateEstimator,
        estimated_update_budget: u64,
    ) -> Self {
        Self {
            update_estimator,
            estimated_update_budget: Some(estimated_update_budget),
            estimated_updates: 0,
        }
    }

    fn should_stop(&self, planned_ranges: usize) -> bool {
        planned_ranges > 0 &&
            self.estimated_update_budget.is_some_and(|budget| self.estimated_updates >= budget)
    }

    fn record_range(&mut self, range: &StoragePrefixRange) {
        self.estimated_updates =
            self.estimated_updates.saturating_add(self.update_estimator.estimate(range));
    }
}

fn plan_storage_prefix<C>(
    cursor: &mut C,
    prefix: Nibbles,
    start_bound: B256,
    config: StoragePrefixPlannerConfig,
    budget: &mut StoragePrefixPlanBudget,
    plan: &mut StoragePrefixPlan,
) -> Result<StoragePrefixPlanStatus, DatabaseError>
where
    C: HashedCursor<Value = U256>,
{
    if budget.should_stop(plan.ranges.len()) {
        return Ok(StoragePrefixPlanStatus::BudgetReached)
    }

    if storage_prefix_end(&prefix).is_some_and(|end| end <= start_bound) {
        return Ok(StoragePrefixPlanStatus::Complete)
    }

    if storage_prefix_bound(&prefix) < start_bound {
        if prefix.len() >= 64 {
            return Ok(StoragePrefixPlanStatus::Complete)
        }
        return plan_storage_prefix_children(cursor, prefix, start_bound, config, budget, plan)
    }

    let probe =
        probe_storage_prefix(cursor, &prefix, config.sample_limit_per_prefix, &mut plan.stats)?;
    if probe.sampled_slots == 0 {
        plan.stats.empty_prefixes += 1;
        return Ok(StoragePrefixPlanStatus::Complete)
    }

    let should_split = probe.sampled_slots >= config.sample_limit_per_prefix &&
        probe.sampled_slots >= config.min_sampled_slots_to_split;
    if !should_split {
        return Ok(push_storage_prefix_range(
            plan,
            prefix,
            probe.sampled_slots,
            probe.sampled_entries,
            config,
            budget,
        ))
    }

    if prefix.len() >= config.max_depth {
        plan.stats.too_deep_prefixes += 1;
        return Ok(push_storage_prefix_range(
            plan,
            prefix,
            probe.sampled_slots,
            probe.sampled_entries,
            config,
            budget,
        ))
    }

    plan_storage_prefix_children(cursor, prefix, start_bound, config, budget, plan)
}

fn plan_storage_prefix_children<C>(
    cursor: &mut C,
    prefix: Nibbles,
    start_bound: B256,
    config: StoragePrefixPlannerConfig,
    budget: &mut StoragePrefixPlanBudget,
    plan: &mut StoragePrefixPlan,
) -> Result<StoragePrefixPlanStatus, DatabaseError>
where
    C: HashedCursor<Value = U256>,
{
    plan.stats.split_prefixes += 1;
    for nibble in 0..16 {
        let mut child_prefix = prefix;
        child_prefix.push(nibble);
        match plan_storage_prefix(cursor, child_prefix, start_bound, config, budget, plan)? {
            StoragePrefixPlanStatus::Complete => {}
            status @ (StoragePrefixPlanStatus::TooManyPrefixes |
            StoragePrefixPlanStatus::BudgetReached) => return Ok(status),
        }
    }

    Ok(StoragePrefixPlanStatus::Complete)
}

fn probe_storage_prefix<C>(
    cursor: &mut C,
    prefix: &Nibbles,
    sample_limit: usize,
    stats: &mut StoragePrefixPlannerStats,
) -> Result<StoragePrefixProbe, DatabaseError>
where
    C: HashedCursor<Value = U256>,
{
    let mut sampled_slots = 0usize;
    let mut sampled_entries = Vec::new();
    let mut entry = cursor.seek(storage_prefix_bound(prefix))?;

    while let Some((hashed_slot, value)) = entry {
        if !Nibbles::unpack(hashed_slot).starts_with(prefix) {
            break;
        }

        sampled_slots += 1;
        sampled_entries.push((hashed_slot, value));
        if sampled_slots >= sample_limit {
            break;
        }
        entry = cursor.next()?;
    }

    stats.probes += 1;
    stats.sampled_slots += sampled_slots;
    stats.max_depth_observed = stats.max_depth_observed.max(prefix.len());

    Ok(StoragePrefixProbe { sampled_slots, sampled_entries })
}

fn push_storage_prefix_range(
    plan: &mut StoragePrefixPlan,
    prefix: Nibbles,
    sampled_slots: usize,
    sampled_entries: Vec<(B256, U256)>,
    config: StoragePrefixPlannerConfig,
    budget: &mut StoragePrefixPlanBudget,
) -> StoragePrefixPlanStatus {
    if plan.ranges.len() >= config.max_prefixes {
        plan.stats.too_many_prefixes += 1;
        return StoragePrefixPlanStatus::TooManyPrefixes
    }

    let range = StoragePrefixRange {
        prefix,
        start: storage_prefix_bound(&prefix),
        end: storage_prefix_end(&prefix),
        sampled_slots,
        sampled_entries,
    };
    budget.record_range(&range);
    plan.stats.reusable_sampled_slots += range.sampled_entries.len();
    plan.ranges.push(range);
    plan.stats.planned_prefixes = plan.ranges.len();
    if budget.should_stop(plan.ranges.len()) {
        StoragePrefixPlanStatus::BudgetReached
    } else {
        StoragePrefixPlanStatus::Complete
    }
}

pub(super) fn storage_prefix_bound(prefix: &Nibbles) -> B256 {
    let mut bound = [0u8; 32];
    let packed = prefix.pack();
    bound[..packed.len()].copy_from_slice(&packed);
    B256::from(bound)
}

pub(super) fn storage_prefix_end(prefix: &Nibbles) -> Option<B256> {
    prefix.next_without_prefix().map(|next_prefix| storage_prefix_bound(&next_prefix))
}

#[derive(Debug, Clone, Copy)]
pub(super) struct SegmentedStorageUpdateEstimator {
    updates_per_slot_milli: u64,
}

impl Default for SegmentedStorageUpdateEstimator {
    fn default() -> Self {
        Self::new()
    }
}

impl SegmentedStorageUpdateEstimator {
    const fn new() -> Self {
        Self { updates_per_slot_milli: 1_500 }
    }

    pub(super) const fn minimum_budget(&self) -> u64 {
        1
    }

    pub(super) fn estimate(&self, range: &StoragePrefixRange) -> u64 {
        ((range.sampled_slots.max(1) as u64) * self.updates_per_slot_milli).div_ceil(1_000).max(1)
    }

    pub(super) fn record(&mut self, walked_slots: usize, updates: usize) {
        if walked_slots == 0 {
            return
        }

        self.updates_per_slot_milli =
            ((updates.max(1) as u64) * 1_000).div_ceil(walked_slots as u64).max(1);
    }
}

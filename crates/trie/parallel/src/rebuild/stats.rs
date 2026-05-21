use std::time::Duration;

use super::{account_prefix::AccountPrefixPlannerStats, storage_prefix::StoragePrefixPlannerStats};

/// Experimental parallel rebuild statistics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StorageRootPrefetchStats {
    pub(super) inline_storage_root_checks: usize,
    pub(super) inline_empty_storage_roots: usize,
    pub(super) inline_small_storage_roots: usize,
    pub(super) inline_small_storage_slots: usize,
    pub(super) inline_storage_root_check_duration: Duration,
    pub(super) storage_root_job_duration: Duration,
    pub(super) serial_storage_root_duration: Duration,
    pub(super) storage_updates_insert_duration: Duration,
    pub(super) account_leaf_duration: Duration,
    pub(super) state_root_finalize_duration: Duration,
    pub(super) storage_presence_checks: usize,
    pub(super) storage_presence_empty_hits: usize,
    pub(super) storage_presence_present_hits: usize,
    pub(super) storage_presence_skipped_keys: usize,
    pub(super) storage_presence_duration: Duration,
    pub(super) storage_progresses: usize,
    pub(super) resumed_storage_progresses: usize,
    pub(super) segmented_storage_attempts: usize,
    pub(super) segmented_storage_roots: usize,
    pub(super) segmented_storage_fallbacks: usize,
    pub(super) segmented_storage_prefixes: usize,
    pub(super) segmented_storage_slots: usize,
    pub(super) segmented_storage_inline_fallbacks: usize,
    pub(super) segmented_storage_partial_plans: usize,
    pub(super) segmented_storage_trigger_progresses: usize,
    pub(super) segmented_storage_trigger_discarded_slots: usize,
    pub(super) segmented_storage_trigger_discarded_updates: usize,
    pub(super) segmented_storage_gate_probes: usize,
    pub(super) segmented_storage_gate_slots: usize,
    pub(super) segmented_storage_gate_count_queries: usize,
    pub(super) segmented_storage_gate_counted_slots: usize,
    pub(super) segmented_storage_gate_duration: Duration,
    pub(super) segmented_storage_gate_hits: usize,
    pub(super) segmented_storage_gate_misses: usize,
    pub(super) segmented_storage_budget_stops: usize,
    pub(super) segmented_storage_resume_fallbacks: usize,
    pub(super) segmented_storage_total_duration: Duration,
    pub(super) segmented_storage_resume_duration: Duration,
    pub(super) segmented_storage_planning_duration: Duration,
    pub(super) segmented_storage_wave_compute_duration: Duration,
    pub(super) segmented_storage_prefix_worker_duration: Duration,
    pub(super) segmented_storage_prefix_merge_duration: Duration,
    pub(super) segmented_storage_serial_fallback_duration: Duration,
    pub(super) segmented_storage_plan_single_range_fallbacks: usize,
    pub(super) segmented_storage_plan_unsupported_prefix_fallbacks: usize,
    pub(super) segmented_storage_plan_splits: usize,
    pub(super) segmented_storage_plan_empty_prefixes: usize,
    pub(super) segmented_storage_plan_planned_prefixes: usize,
    pub(super) segmented_storage_plan_max_depth_observed: usize,
    pub(super) segmented_storage_plan_too_many_prefixes: usize,
    pub(super) segmented_storage_plan_too_deep_prefixes: usize,
    pub(super) segmented_storage_plan_probes: usize,
    pub(super) segmented_storage_plan_sampled_slots: usize,
    pub(super) segmented_storage_plan_reusable_sampled_slots: usize,
    pub(super) segmented_storage_prefix_cached_slots: usize,
    pub(super) segmented_storage_prefix_unsupported_root_fallbacks: usize,
    pub(super) segmented_storage_prefix_empty_range_fallbacks: usize,
    pub(super) segmented_storage_missing_prefix_result_fallbacks: usize,
    pub(super) account_prefix_planned_ranges: usize,
    pub(super) account_prefix_planned_items: usize,
    pub(super) account_prefix_planned_accounts: usize,
    pub(super) account_prefix_storage_presence_checks: usize,
    pub(super) account_prefix_windows: usize,
    pub(super) account_prefix_completed_ranges: usize,
    pub(super) account_prefix_boundary_checkpoints: usize,
    pub(super) account_prefix_large_storage_barriers: usize,
    pub(super) account_prefix_completed_barriers: usize,
    pub(super) account_prefix_storage_progresses: usize,
    pub(super) account_prefix_storage_empty_hits: usize,
    pub(super) account_prefix_storage_present_hits: usize,
    pub(super) account_prefix_storage_skipped_keys: usize,
    pub(super) account_prefix_storage_count_queries: usize,
    pub(super) account_prefix_storage_counted_slots: usize,
    pub(super) account_prefix_max_account_storage_slots: usize,
    pub(super) account_prefix_large_storage_accounts: usize,
    pub(super) account_prefix_total_weight: u64,
    pub(super) account_prefix_max_range_weight: u64,
    pub(super) account_prefix_planning_duration: Duration,
    pub(super) account_prefix_window_compute_duration: Duration,
    pub(super) account_prefix_merge_duration: Duration,
}

impl StorageRootPrefetchStats {
    pub(super) fn extend(&mut self, other: Self) {
        self.inline_storage_root_checks += other.inline_storage_root_checks;
        self.inline_empty_storage_roots += other.inline_empty_storage_roots;
        self.inline_small_storage_roots += other.inline_small_storage_roots;
        self.inline_small_storage_slots += other.inline_small_storage_slots;
        self.inline_storage_root_check_duration += other.inline_storage_root_check_duration;
        self.storage_root_job_duration += other.storage_root_job_duration;
        self.serial_storage_root_duration += other.serial_storage_root_duration;
        self.storage_updates_insert_duration += other.storage_updates_insert_duration;
        self.account_leaf_duration += other.account_leaf_duration;
        self.state_root_finalize_duration += other.state_root_finalize_duration;
        self.storage_presence_checks += other.storage_presence_checks;
        self.storage_presence_empty_hits += other.storage_presence_empty_hits;
        self.storage_presence_present_hits += other.storage_presence_present_hits;
        self.storage_presence_skipped_keys += other.storage_presence_skipped_keys;
        self.storage_presence_duration += other.storage_presence_duration;
        self.storage_progresses += other.storage_progresses;
        self.resumed_storage_progresses += other.resumed_storage_progresses;
        self.segmented_storage_attempts += other.segmented_storage_attempts;
        self.segmented_storage_roots += other.segmented_storage_roots;
        self.segmented_storage_fallbacks += other.segmented_storage_fallbacks;
        self.segmented_storage_prefixes += other.segmented_storage_prefixes;
        self.segmented_storage_slots += other.segmented_storage_slots;
        self.segmented_storage_inline_fallbacks += other.segmented_storage_inline_fallbacks;
        self.segmented_storage_partial_plans += other.segmented_storage_partial_plans;
        self.segmented_storage_trigger_progresses += other.segmented_storage_trigger_progresses;
        self.segmented_storage_trigger_discarded_slots +=
            other.segmented_storage_trigger_discarded_slots;
        self.segmented_storage_trigger_discarded_updates +=
            other.segmented_storage_trigger_discarded_updates;
        self.segmented_storage_gate_probes += other.segmented_storage_gate_probes;
        self.segmented_storage_gate_slots += other.segmented_storage_gate_slots;
        self.segmented_storage_gate_count_queries += other.segmented_storage_gate_count_queries;
        self.segmented_storage_gate_counted_slots += other.segmented_storage_gate_counted_slots;
        self.segmented_storage_gate_duration += other.segmented_storage_gate_duration;
        self.segmented_storage_gate_hits += other.segmented_storage_gate_hits;
        self.segmented_storage_gate_misses += other.segmented_storage_gate_misses;
        self.segmented_storage_budget_stops += other.segmented_storage_budget_stops;
        self.segmented_storage_resume_fallbacks += other.segmented_storage_resume_fallbacks;
        self.segmented_storage_total_duration += other.segmented_storage_total_duration;
        self.segmented_storage_resume_duration += other.segmented_storage_resume_duration;
        self.segmented_storage_planning_duration += other.segmented_storage_planning_duration;
        self.segmented_storage_wave_compute_duration +=
            other.segmented_storage_wave_compute_duration;
        self.segmented_storage_prefix_worker_duration +=
            other.segmented_storage_prefix_worker_duration;
        self.segmented_storage_prefix_merge_duration +=
            other.segmented_storage_prefix_merge_duration;
        self.segmented_storage_serial_fallback_duration +=
            other.segmented_storage_serial_fallback_duration;
        self.segmented_storage_plan_single_range_fallbacks +=
            other.segmented_storage_plan_single_range_fallbacks;
        self.segmented_storage_plan_unsupported_prefix_fallbacks +=
            other.segmented_storage_plan_unsupported_prefix_fallbacks;
        self.segmented_storage_plan_splits += other.segmented_storage_plan_splits;
        self.segmented_storage_plan_empty_prefixes += other.segmented_storage_plan_empty_prefixes;
        self.segmented_storage_plan_planned_prefixes +=
            other.segmented_storage_plan_planned_prefixes;
        self.segmented_storage_plan_max_depth_observed = self
            .segmented_storage_plan_max_depth_observed
            .max(other.segmented_storage_plan_max_depth_observed);
        self.segmented_storage_plan_too_many_prefixes +=
            other.segmented_storage_plan_too_many_prefixes;
        self.segmented_storage_plan_too_deep_prefixes +=
            other.segmented_storage_plan_too_deep_prefixes;
        self.segmented_storage_plan_probes += other.segmented_storage_plan_probes;
        self.segmented_storage_plan_sampled_slots += other.segmented_storage_plan_sampled_slots;
        self.segmented_storage_plan_reusable_sampled_slots +=
            other.segmented_storage_plan_reusable_sampled_slots;
        self.segmented_storage_prefix_cached_slots += other.segmented_storage_prefix_cached_slots;
        self.segmented_storage_prefix_unsupported_root_fallbacks +=
            other.segmented_storage_prefix_unsupported_root_fallbacks;
        self.segmented_storage_prefix_empty_range_fallbacks +=
            other.segmented_storage_prefix_empty_range_fallbacks;
        self.segmented_storage_missing_prefix_result_fallbacks +=
            other.segmented_storage_missing_prefix_result_fallbacks;
        self.account_prefix_planned_ranges += other.account_prefix_planned_ranges;
        self.account_prefix_planned_items += other.account_prefix_planned_items;
        self.account_prefix_planned_accounts += other.account_prefix_planned_accounts;
        self.account_prefix_storage_presence_checks += other.account_prefix_storage_presence_checks;
        self.account_prefix_windows += other.account_prefix_windows;
        self.account_prefix_completed_ranges += other.account_prefix_completed_ranges;
        self.account_prefix_boundary_checkpoints += other.account_prefix_boundary_checkpoints;
        self.account_prefix_large_storage_barriers += other.account_prefix_large_storage_barriers;
        self.account_prefix_completed_barriers += other.account_prefix_completed_barriers;
        self.account_prefix_storage_progresses += other.account_prefix_storage_progresses;
        self.account_prefix_storage_empty_hits += other.account_prefix_storage_empty_hits;
        self.account_prefix_storage_present_hits += other.account_prefix_storage_present_hits;
        self.account_prefix_storage_skipped_keys += other.account_prefix_storage_skipped_keys;
        self.account_prefix_storage_count_queries += other.account_prefix_storage_count_queries;
        self.account_prefix_storage_counted_slots += other.account_prefix_storage_counted_slots;
        self.account_prefix_max_account_storage_slots = self
            .account_prefix_max_account_storage_slots
            .max(other.account_prefix_max_account_storage_slots);
        self.account_prefix_large_storage_accounts += other.account_prefix_large_storage_accounts;
        self.account_prefix_total_weight += other.account_prefix_total_weight;
        self.account_prefix_max_range_weight =
            self.account_prefix_max_range_weight.max(other.account_prefix_max_range_weight);
        self.account_prefix_planning_duration += other.account_prefix_planning_duration;
        self.account_prefix_window_compute_duration += other.account_prefix_window_compute_duration;
        self.account_prefix_merge_duration += other.account_prefix_merge_duration;
    }

    pub(super) fn record_account_prefix_plan(&mut self, plan: &AccountPrefixPlannerStats) {
        self.account_prefix_planned_ranges += plan.planned_ranges;
        self.account_prefix_planned_items += plan.planned_items;
        self.account_prefix_planned_accounts += plan.accounts;
        self.account_prefix_storage_presence_checks += plan.storage_presence_checks;
        self.account_prefix_storage_empty_hits += plan.storage_empty_hits;
        self.account_prefix_storage_present_hits += plan.storage_present_hits;
        self.account_prefix_storage_skipped_keys += plan.storage_skipped_keys;
        self.account_prefix_storage_count_queries += plan.storage_count_queries;
        self.account_prefix_storage_counted_slots += plan.storage_counted_slots;
        self.account_prefix_max_account_storage_slots =
            self.account_prefix_max_account_storage_slots.max(plan.max_account_storage_slots);
        self.account_prefix_large_storage_accounts += plan.large_storage_accounts;
        self.account_prefix_large_storage_barriers += plan.large_storage_barriers;
        self.account_prefix_total_weight += plan.total_weight;
        self.account_prefix_max_range_weight =
            self.account_prefix_max_range_weight.max(plan.max_range_weight);
        self.account_prefix_planning_duration += plan.planning_duration;
    }

    pub(super) fn record_storage_prefix_plan(&mut self, plan: &StoragePrefixPlannerStats) {
        self.segmented_storage_plan_splits += plan.split_prefixes;
        self.segmented_storage_plan_empty_prefixes += plan.empty_prefixes;
        self.segmented_storage_plan_planned_prefixes += plan.planned_prefixes;
        self.segmented_storage_plan_max_depth_observed =
            self.segmented_storage_plan_max_depth_observed.max(plan.max_depth_observed);
        self.segmented_storage_plan_too_many_prefixes += plan.too_many_prefixes;
        self.segmented_storage_plan_too_deep_prefixes += plan.too_deep_prefixes;
        self.segmented_storage_plan_probes += plan.probes;
        self.segmented_storage_plan_sampled_slots += plan.sampled_slots;
        self.segmented_storage_plan_reusable_sampled_slots += plan.reusable_sampled_slots;
    }
}

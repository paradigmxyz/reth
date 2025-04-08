//! Interop metrics.

use crate::InvalidCrossTx;
use core::sync::atomic::{AtomicUsize, Ordering};
use metrics::Histogram;
use op_alloy_consensus::interop::SafetyLevel;
use reth_metrics::Metrics;

/// Metrics tracking [`SupervisorClient`](crate::SupervisorClient) performance.
#[derive(Debug, Default)]
pub struct SupervisorMetrics {
    /// Frequency of each [`SafetyLevel`] among cross chain transactions that fail validation.
    pub(crate) failed_safety_level_metrics: FailedSafetyLevelMetrics,
    /// Intermediary [`SafetyLevelCounter`], helper for batch updating
    /// [`failed_safety_level_metrics`](Self::failed_safety_level_metrics).
    pub(crate) safety_level_counter: SafetyLevelCounter,
    // todo: add RPC latency metrics
}

impl SupervisorMetrics {
    /// Updates metrics for a batch of transactions from given intermediary [`SafetyLevelCounter`].
    pub(crate) fn update(&self) {
        let Self { failed_safety_level_metrics, safety_level_counter } = self;

        failed_safety_level_metrics.update_batch(safety_level_counter);
    }

    /// Count [`SafetyLevel`] if given error is
    /// [`MinimumSafety`](crate::InvalidInboxEntry::MinimumSafety) error.
    ///
    /// NOTE: Does _not_ write to metrics. Writing updates must explicitly be done calling
    /// [`update`](Self::update) after this call.
    pub(crate) fn maybe_increment_safety_level_counter(&self, err: &InvalidCrossTx) {
        if let Some(denied_level) = err.msg_safety_level() {
            self.safety_level_counter.increase_safety_level_count_once_for(denied_level);
        }
    }
}

/// Tracks the safety level of transactions that didn't pass validation, i.e. for which RPC
/// `supervisor_checkAccessList` returned error
/// [`InvalidInboxEntry::MinimumSafetyLevel`](crate::InvalidInboxEntry::MinimumSafetyLevel).
#[derive(Metrics)]
#[metrics(scope = "transaction_pool.supervisor")]
pub struct FailedSafetyLevelMetrics {
    /// Frequency of cross chain transactions failing validation due to being
    /// [`SafetyLevel::Invalid`](SafetyLevel::Invalid).
    pub(crate) invalid: Histogram,

    /// Frequency of cross chain transactions failing validation due to being
    /// [`SafetyLevel::Unsafe`](SafetyLevel::Unsafe).
    pub(crate) r#unsafe: Histogram,

    /// Frequency of cross chain transactions failing validation due to being
    /// [`SafetyLevel::CrossUnsafe`](SafetyLevel::CrossUnsafe).
    pub(crate) cross_unsafe: Histogram,

    /// Frequency of cross chain transactions failing validation due to being
    /// [`SafetyLevel::LocalSafe`](SafetyLevel::LocalSafe).
    pub(crate) local_safe: Histogram,

    /// Frequency of cross chain transactions failing validation due to being
    /// [`SafetyLevel::Safe`](SafetyLevel::Safe).
    pub(crate) safe: Histogram,
}

impl FailedSafetyLevelMetrics {
    /// Updates metrics for a batch of transactions from given intermediary [`SafetyLevelCounter`].
    pub(crate) fn update_batch(&self, safety_level_counter: &SafetyLevelCounter) {
        let SafetyLevelCounter { invalid, r#unsafe, cross_unsafe, local_safe, safe } =
            safety_level_counter;

        self.invalid.record(invalid.swap(0, Ordering::Relaxed) as f64);
        self.r#unsafe.record(r#unsafe.swap(0, Ordering::Relaxed) as f64);
        self.cross_unsafe.record(cross_unsafe.swap(0, Ordering::Relaxed) as f64);
        self.local_safe.record(local_safe.swap(0, Ordering::Relaxed) as f64);
        self.safe.record(safe.swap(0, Ordering::Relaxed) as f64);
    }
}

/// Helper type for batch updating [`FailedSafetyLevelMetrics`].
///
/// Counts number of cross chain transactions, by their [`SafetyLevel`], that
/// fail validation due to not meeting locally configured minium safety level requirement.
#[derive(Debug, Default)]
pub struct SafetyLevelCounter {
    /// Count of cross chain transactions failing validation due to being
    /// [`Invalid`](SafetyLevel::Invalid).
    pub(crate) invalid: AtomicUsize,

    /// Count of cross chain transactions failing validation due to being
    /// [`Unsafe`](SafetyLevel::Unsafe).
    pub(crate) r#unsafe: AtomicUsize,

    /// Count of cross chain transactions failing validation due to being
    /// [`CrossUnsafe`](SafetyLevel::CrossUnsafe).
    pub(crate) cross_unsafe: AtomicUsize,

    /// Count of cross chain transactions failing validation due to being
    /// [`LocalSafe`](SafetyLevel::LocalSafe).
    pub(crate) local_safe: AtomicUsize,

    /// Count of cross chain transactions failing validation due to being
    /// [`Safe`](SafetyLevel::Safe).
    pub(crate) safe: AtomicUsize,
}

impl SafetyLevelCounter {
    /// Increases counter for given [`SafetyLevel`].
    pub(crate) fn increase_safety_level_count_once_for(&self, level: SafetyLevel) {
        if matches!(level, SafetyLevel::Invalid) {
            self.invalid.fetch_add(1, Ordering::Relaxed);
        } else if matches!(level, SafetyLevel::Unsafe) {
            self.r#unsafe.fetch_add(1, Ordering::Relaxed);
        } else if matches!(level, SafetyLevel::CrossUnsafe) {
            self.cross_unsafe.fetch_add(1, Ordering::Relaxed);
        } else if matches!(level, SafetyLevel::LocalSafe) {
            self.local_safe.fetch_add(1, Ordering::Relaxed);
        } else if matches!(level, SafetyLevel::Safe) {
            self.safe.fetch_add(1, Ordering::Relaxed);
        }
    }
}

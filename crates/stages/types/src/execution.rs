use std::time::Duration;

/// The thresholds at which the execution stage writes state changes to the database.
///
/// If either of the thresholds (`max_blocks` and `max_changes`) are hit, then the execution stage
/// commits all pending changes to the database.
///
/// A third threshold, `max_changesets`, can be set to periodically write changesets to the
/// current database transaction, which frees up memory.
#[derive(Debug, Clone)]
pub struct ExecutionStageThresholds {
    /// The maximum number of blocks to execute before the execution stage commits.
    pub max_blocks: Option<u64>,
    /// The maximum number of state changes to keep in memory before the execution stage commits.
    pub max_changes: Option<u64>,
    /// The maximum cumulative amount of gas to process before the execution stage commits.
    pub max_cumulative_gas: Option<u64>,
    /// The maximum spent on blocks processing before the execution stage commits.
    pub max_duration: Option<Duration>,
}

impl Default for ExecutionStageThresholds {
    fn default() -> Self {
        Self {
            max_blocks: Some(500_000),
            max_changes: Some(5_000_000),
            // 50k full blocks of 30M gas
            max_cumulative_gas: Some(30_000_000 * 50_000),
            // 10 minutes
            max_duration: Some(Duration::from_secs(10 * 60)),
        }
    }
}

impl ExecutionStageThresholds {
    /// Check if the batch thresholds have been hit.
    #[inline]
    pub fn is_end_of_batch(
        &self,
        blocks_processed: u64,
        changes_processed: u64,
        cumulative_gas_used: u64,
        elapsed: Duration,
    ) -> bool {
        blocks_processed >= self.max_blocks.unwrap_or(u64::MAX) ||
            changes_processed >= self.max_changes.unwrap_or(u64::MAX) ||
            cumulative_gas_used >= self.max_cumulative_gas.unwrap_or(u64::MAX) ||
            elapsed >= self.max_duration.unwrap_or(Duration::MAX)
    }
}

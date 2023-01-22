//! Config structs used to configure each sync stage.

// These are copied from bin/reth/src/config.rs so we can have the same defaults / stage
// configuration values
// TODO: keep an eye on config structs being introduced to reth-cli-utils

/// Configuration for each stage in the pipeline.
#[derive(Debug, Clone, Default)]
pub(crate) struct StageConfig {
    /// Total difficulty stage configuration
    pub(crate) total_difficulty: TotalDifficultyConfig,
    /// Body stage configuration.
    pub(crate) bodies: BodiesConfig,
    /// Sender recovery stage configuration.
    pub(crate) sender_recovery: SenderRecoveryConfig,
    /// Execution stage configuration.
    pub(crate) execution: ExecutionConfig,
}

/// Total difficulty stage configuration
#[derive(Debug, Clone)]
pub(crate) struct TotalDifficultyConfig {
    /// The maximum number of total difficulty entries to sum up before committing progress to the
    /// database.
    pub(crate) commit_threshold: u64,
}

impl Default for TotalDifficultyConfig {
    fn default() -> Self {
        Self { commit_threshold: 100_000 }
    }
}

/// Body stage configuration.
#[derive(Debug, Clone)]
pub(crate) struct BodiesConfig {
    /// The maximum number of bodies to download before committing progress to the database.
    pub(crate) commit_threshold: u64,
}

impl Default for BodiesConfig {
    fn default() -> Self {
        Self { commit_threshold: 5_000 }
    }
}

/// Sender recovery stage configuration.
#[derive(Debug, Clone)]
pub(crate) struct SenderRecoveryConfig {
    /// The maximum number of blocks to process before committing progress to the database.
    pub(crate) commit_threshold: u64,
    /// The maximum number of transactions to recover senders for concurrently.
    pub(crate) batch_size: usize,
}

impl Default for SenderRecoveryConfig {
    fn default() -> Self {
        Self { commit_threshold: 5_000, batch_size: 1000 }
    }
}

/// Execution stage configuration.
#[derive(Debug, Clone)]
pub(crate) struct ExecutionConfig {
    /// The maximum number of blocks to execution before committing progress to the database.
    pub(crate) commit_threshold: u64,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self { commit_threshold: 5_000 }
    }
}

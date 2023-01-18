//! Config structs used to configure each sync stage.

// These are copied from bin/reth/src/config.rs so we can have the same defaults / stage
// configuration values
// TODO: keep an eye on config structs being introduced to reth-cli-utils

/// Configuration for each stage in the pipeline.
#[derive(Debug, Clone, Default)]
pub(crate) struct StageConfig {
    /// Header stage configuration.
    pub(crate) headers: HeadersConfig,
    /// Total difficulty stage configuration
    pub(crate) total_difficulty: TotalDifficultyConfig,
    /// Body stage configuration.
    pub(crate) bodies: BodiesConfig,
    /// Sender recovery stage configuration.
    pub(crate) sender_recovery: SenderRecoveryConfig,
    /// Execution stage configuration.
    pub(crate) execution: ExecutionConfig,
}

/// Header stage configuration.
#[derive(Debug, Clone)]
pub(crate) struct HeadersConfig {
    /// The maximum number of headers to download before committing progress to the database.
    pub(crate) commit_threshold: u64,
    /// The maximum number of headers to request from a peer at a time.
    pub(crate) downloader_batch_size: u64,
    /// The number of times to retry downloading a set of headers.
    pub(crate) downloader_retries: usize,
}

impl Default for HeadersConfig {
    fn default() -> Self {
        Self { commit_threshold: 10_000, downloader_batch_size: 1000, downloader_retries: 5 }
    }
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
    /// The maximum number of bodies to request from a peer at a time.
    pub(crate) downloader_batch_size: usize,
    /// The number of times to retry downloading a set of bodies.
    pub(crate) downloader_retries: usize,
    /// The maximum number of body requests to have in flight at a time.
    ///
    /// The maximum number of bodies downloaded at the same time is `downloader_batch_size *
    /// downloader_concurrency`.
    pub(crate) downloader_concurrency: usize,
}

impl Default for BodiesConfig {
    fn default() -> Self {
        Self {
            commit_threshold: 5_000,
            downloader_batch_size: 100,
            downloader_retries: 5,
            downloader_concurrency: 10,
        }
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

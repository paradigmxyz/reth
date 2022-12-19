//! Configuration files.
use serde::{Deserialize, Serialize};

/// Configuration for the reth node.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Config {
    /// Configuration for each stage in the pipeline.
    // TODO(onbjerg): Can we make this easier to maintain when we add/remove stages?
    pub stages: StageConfig,
}

/// Configuration for each stage in the pipeline.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct StageConfig {
    /// Header stage configuration.
    pub headers: HeadersConfig,
    /// Body stage configuration.
    pub bodies: BodiesConfig,
    /// Sender recovery stage configuration.
    pub senders: SendersConfig,
}

/// Header stage configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HeadersConfig {
    /// The maximum number of headers to download before committing progress to the database.
    pub commit_threshold: u64,
    /// The maximum number of headers to request from a peer at a time.
    pub downloader_batch_size: u64,
    /// The number of times to retry downloading a set of headers.
    pub downloader_retries: usize,
}

impl Default for HeadersConfig {
    fn default() -> Self {
        Self { commit_threshold: 10_000, downloader_batch_size: 1000, downloader_retries: 5 }
    }
}

/// Body stage configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BodiesConfig {
    /// The maximum number of bodies to download before committing progress to the database.
    pub commit_threshold: u64,
    /// The maximum number of bodies to request from a peer at a time.
    pub downloader_batch_size: usize,
    /// The number of times to retry downloading a set of bodies.
    pub downloader_retries: usize,
    /// The maximum number of body requests to have in flight at a time.
    ///
    /// The maximum number of bodies downloaded at the same time is `downloader_batch_size *
    /// downloader_concurrency`.
    pub downloader_concurrency: usize,
}

impl Default for BodiesConfig {
    fn default() -> Self {
        Self {
            commit_threshold: 5_000,
            downloader_batch_size: 200,
            downloader_retries: 5,
            downloader_concurrency: 10,
        }
    }
}

/// Sender recovery stage configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SendersConfig {
    /// The maximum number of blocks to process before committing progress to the database.
    pub commit_threshold: u64,
    /// The maximum number of transactions to recover senders for concurrently.
    pub batch_size: usize,
}

impl Default for SendersConfig {
    fn default() -> Self {
        Self { commit_threshold: 5_000, batch_size: 1000 }
    }
}

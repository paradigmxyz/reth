use super::progress::{DownloadRequestLimiter, SharedProgress};
use eyre::Result;
use reth_cli_util::cancellation::CancellationToken;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

/// Shared state for one run of `reth download`.
#[derive(Clone)]
pub(crate) struct DownloadSession {
    /// Shared progress counters for this command, when enabled.
    progress: Option<Arc<SharedProgress>>,
    /// Shared limit for concurrent HTTP requests, when enabled.
    request_limiter: Option<Arc<DownloadRequestLimiter>>,
    /// Cancellation token shared by the whole command.
    cancel_token: CancellationToken,
    /// Optional fixed delay overriding the default retry backoff.
    retry_backoff: Option<Duration>,
}

impl DownloadSession {
    /// Stores the shared progress, request limiter, and cancellation token.
    pub(crate) fn new(
        progress: Option<Arc<SharedProgress>>,
        request_limiter: Option<Arc<DownloadRequestLimiter>>,
        cancel_token: CancellationToken,
    ) -> Self {
        Self { progress, request_limiter, cancel_token, retry_backoff: None }
    }

    /// Overrides the delay between retry attempts for this download.
    pub(crate) const fn with_retry_backoff(mut self, retry_backoff: Option<Duration>) -> Self {
        self.retry_backoff = retry_backoff;
        self
    }

    /// Returns the configured delay, preserving the caller's default when no override is set.
    pub(crate) fn retry_delay(&self, default: Duration) -> Duration {
        self.retry_backoff.unwrap_or(default)
    }

    /// Returns the shared progress tracker, if this flow uses one.
    pub(crate) fn progress(&self) -> Option<&Arc<SharedProgress>> {
        self.progress.as_ref()
    }

    /// Returns the shared HTTP request limiter, if this flow uses one.
    pub(crate) fn request_limiter(&self) -> Option<&Arc<DownloadRequestLimiter>> {
        self.request_limiter.as_ref()
    }

    /// Returns the request limiter or errors if the caller needs one.
    pub(crate) fn require_request_limiter(&self) -> Result<&Arc<DownloadRequestLimiter>> {
        self.request_limiter().ok_or_else(|| eyre::eyre!("Missing download request limiter"))
    }

    /// Returns the cancellation token for this command.
    pub(crate) fn cancel_token(&self) -> &CancellationToken {
        &self.cancel_token
    }

    /// Records one archive whose outputs were already reusable on disk.
    pub(crate) fn record_reused_archive(&self, download_bytes: u64, output_bytes: u64) {
        if let Some(progress) = self.progress() {
            progress.record_reused_archive(download_bytes, output_bytes);
        }
    }

    /// Records one archive whose extracted outputs fully verified.
    pub(crate) fn record_archive_output_complete(&self, bytes: u64) {
        if let Some(progress) = self.progress() {
            progress.record_archive_output_complete(bytes);
        }
    }
}

/// Paths used while processing one archive, plus the shared download session.
#[derive(Clone)]
pub(crate) struct ArchiveProcessContext {
    /// Directory where extracted output files are written.
    target_dir: PathBuf,
    /// Directory used for cached archive downloads, when enabled.
    cache_dir: Option<PathBuf>,
    /// Shared command-scoped download state.
    session: DownloadSession,
}

impl ArchiveProcessContext {
    /// Creates the context used while processing modular archives.
    pub(crate) fn new(
        target_dir: PathBuf,
        cache_dir: Option<PathBuf>,
        session: DownloadSession,
    ) -> Self {
        Self { target_dir, cache_dir, session }
    }

    /// Returns the directory where extracted outputs should be written.
    pub(crate) fn target_dir(&self) -> &Path {
        &self.target_dir
    }

    /// Returns the cache directory for two-phase downloads, if enabled.
    pub(crate) fn cache_dir(&self) -> Option<&Path> {
        self.cache_dir.as_deref()
    }

    /// Returns the shared download session.
    pub(crate) fn session(&self) -> &DownloadSession {
        &self.session
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_delay_preserves_defaults_or_uses_override() {
        let session = DownloadSession::new(None, None, CancellationToken::new());
        for default in [Duration::from_secs(2), Duration::from_secs(5), Duration::from_secs(40)] {
            assert_eq!(session.retry_delay(default), default);
            for delay in [Duration::ZERO, Duration::from_millis(250)] {
                assert_eq!(
                    session.clone().with_retry_backoff(Some(delay)).retry_delay(default),
                    delay
                );
            }
        }
    }
}

use metrics::Counter;
use reth_interfaces::p2p::error::DownloadError;
use reth_metrics_derive::Metrics;

/// The header downloader metrics.
#[derive(Metrics)]
#[metrics(scope = "downloaders_headers")]
pub struct HeaderDownloaderMetrics {
    /// The number of headers that were successfully returned by the downloader.
    pub(crate) total_returned: Counter,
    /// Number of headers that were successfully downloaded
    pub(crate) total_downloaded: Counter,
    /// Number of timeout errors while requesting headers
    timeout_errors: Counter,
    /// Number of validation errors while requesting headers
    validation_errors: Counter,
    /// Number of unexpected errors while requesting headers
    unexpected_errors: Counter,
}

impl HeaderDownloaderMetrics {
    /// Increment errors counter.
    pub(crate) fn increment_errors(&self, error: &DownloadError) {
        match error {
            DownloadError::Timeout => self.timeout_errors.increment(1),
            DownloadError::HeaderValidation { .. } => self.validation_errors.increment(1),
            _error => self.unexpected_errors.increment(1),
        }
    }
}

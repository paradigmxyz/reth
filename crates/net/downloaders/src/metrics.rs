use metrics::Counter;
use reth_interfaces::p2p::error::DownloadError;
use reth_metrics_derive::Metrics;

/// Common downloader metrics.
///
/// These metrics will be dynamically initialized with the provided scope
/// by corresponding downloaders.
/// ```
/// use reth_downloaders::metrics::DownloaderMetrics;
/// use reth_interfaces::p2p::error::DownloadError;
///
/// // Initialize metrics.
/// let metrics = DownloaderMetrics::new("downloaders.headers");
/// // Increment `downloaders.headers.timeout_errors` counter by 1.
/// metrics.increment_errors(&DownloadError::Timeout);
/// ```
#[derive(Clone, Metrics)]
#[metrics(dynamic = true)]
pub struct DownloaderMetrics {
    /// The number of items that were successfully sent to the poller (stage)
    pub total_flushed: Counter,
    /// Number of items that were successfully downloaded
    pub total_downloaded: Counter,
    /// Number of timeout errors while requesting items
    pub timeout_errors: Counter,
    /// Number of validation errors while requesting items
    pub validation_errors: Counter,
    /// Number of unexpected errors while requesting items
    pub unexpected_errors: Counter,
}

impl DownloaderMetrics {
    /// Increment errors counter.
    pub fn increment_errors(&self, error: &DownloadError) {
        match error {
            DownloadError::Timeout => self.timeout_errors.increment(1),
            DownloadError::HeaderValidation { .. } | DownloadError::BodyValidation { .. } => {
                self.validation_errors.increment(1)
            }
            _error => self.unexpected_errors.increment(1),
        }
    }
}

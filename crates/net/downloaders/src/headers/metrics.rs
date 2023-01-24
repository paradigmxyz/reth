use metrics::Counter;
use reth_interfaces::p2p::error::DownloadError;
use reth_metrics_derive::Metrics;

/// The header downloader metrics.
#[derive(Metrics)]
#[metrics(scope = "downloaders_headers")]
pub struct HeaderDownloaderMetrics {
    /// The number of headers that were successfully returned by the downloader.
    total_returned: Counter,
    /// Number of headers that were successfully downloaded
    total_downloaded: Counter,
    /// Number of timeout errors while requesting headers
    timeout_errors: Counter,
    /// Number of validation errors while requesting headers
    validation_errors: Counter,
    /// Number of unexpected errors while requesting headers
    unexpected_errors: Counter,
}

impl HeaderDownloaderMetrics {
    /// Increment the number of total downloaded headers.
    pub(crate) fn update_total_returned(&self, number: usize) {
        self.total_returned.increment(number as u64)
    }

    /// Increment the number of total downloaded headers.
    pub(crate) fn update_total_downloaded(&self, number: usize) {
        self.total_downloaded.increment(number as u64)
    }

    /// Update header errors metrics
    pub(crate) fn update_error_metrics(&self, error: &DownloadError) {
        match error {
            DownloadError::Timeout => self.timeout_errors.increment(1),
            DownloadError::HeaderValidation { .. } => self.validation_errors.increment(1),
            _error => self.unexpected_errors.increment(1),
        }
    }
}

use metrics::Counter;
use reth_interfaces::p2p::error::DownloadError;
use reth_metrics_derive::Metrics;

/// Stagedsync header metrics
#[derive(Metrics)]
#[metrics(scope = "stages.headers")]
pub struct HeaderMetrics {
    /// Number of headers successfully retrieved
    pub headers_counter: Counter,
    /// Number of timeout errors while requesting headers
    pub timeout_errors: Counter,
    /// Number of validation errors while requesting headers
    pub validation_errors: Counter,
    /// Number of unexpected errors while requesting headers
    pub unexpected_errors: Counter,
}

impl HeaderMetrics {
    /// Update header errors metrics
    pub fn update_headers_error_metrics(&self, error: &DownloadError) {
        match error {
            DownloadError::Timeout => self.timeout_errors.increment(1),
            DownloadError::HeaderValidation { hash: _, error: _ } => {
                self.validation_errors.increment(1)
            }
            _error => self.unexpected_errors.increment(1),
        }
    }
}

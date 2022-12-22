use metrics::{register_counter, Counter};
use reth_interfaces::p2p::error::DownloadError;
use std::fmt;

/// Stagedsync header metrics
pub struct HeaderMetrics {
    /// Number of headers successfully retrieved
    pub headers_counter: Counter,
    /// Number of timeout errors while requesting headers
    pub headers_timeout_errors: Counter,
    /// Number of validation errors while requesting headers
    pub headers_validation_errors: Counter,
    /// Elapsed time of successful header requests
    pub headers_unexpected_errors: Counter,
}

impl HeaderMetrics {
    /// Update header errors metrics
    pub fn update_headers_error_metrics(&self, error: &DownloadError) {
        match error {
            DownloadError::Timeout => self.headers_timeout_errors.increment(1),
            DownloadError::HeaderValidation { hash: _, error: _ } => {
                self.headers_validation_errors.increment(1)
            }
            _error => self.headers_unexpected_errors.increment(1),
        }
    }
}

impl Default for HeaderMetrics {
    /// Initialize header metrics struct and register them
    fn default() -> Self {
        Self {
            headers_counter: register_counter!("stages.headers.counter"),
            headers_timeout_errors: register_counter!("stages.headers.timeout_errors"),
            headers_validation_errors: register_counter!("stages.headers.validation_errors"),
            headers_unexpected_errors: register_counter!("stages.headers.unexpected_errors"),
        }
    }
}

impl fmt::Debug for HeaderMetrics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HeaderMetrics").finish()
    }
}

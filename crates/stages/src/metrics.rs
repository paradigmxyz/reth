use metrics::{describe_counter, describe_histogram, register_counter, Counter};
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
    /// Number of unexpected errors while requesting headers
    pub headers_unexpected_errors: Counter,
}

impl HeaderMetrics {
    /// Describe stagedsync headers metrics
    pub fn describe() {
        describe_counter!("stages.headers.counter", "Number of headers successfully retrieved");
        describe_counter!(
            "stages.headers.timeout_errors",
            "Number of timeout errors while requesting headers"
        );
        describe_counter!(
            "stages.headers.validation_errors",
            "Number of validation errors while requesting headers"
        );
        describe_counter!(
            "stages.headers.unexpected_errors",
            "Number of unexpected errors while requesting headers"
        );
        describe_histogram!(
            "stages.headers.request_time",
            "Elapsed time of successful header requests"
        );
    }

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

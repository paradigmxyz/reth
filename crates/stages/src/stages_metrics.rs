use metrics::{counter, increment_counter, Counter};
use reth_interfaces::p2p::error::DownloadError;

struct HeaderMetrics {
    headers_counter: Counter,
    headers_timeout_errors: Counter,
    headers_validation_errors: Counter,
    headers_unexpected_errors: Counter,
}

impl HeaderMetrics {
    pub fn new() -> Self {
        Self {
            headers_counter: register_counter!("stages.headers.counter"),
            headers_timeout_errors: register_counter!("stages.headers.timeout_errors"),
            headers_validation_errors: register_counter!("stages.headers.validation_errors"),
            headers_unexpected_errors: register_counter!("stages.headers.unexpected_errors"),
        }
    }

    pub fn update_headers_errors(&self, error: &DownloadError) {
        match error {
            DownloadError::Timeout => self.headers_timeout_errors.increment(1),
            DownloadError::HeaderValidation { hash: _, error: _ } => {
                self.headers_validation_errors.increment(1)
            }
            _error => self.headers_unexpected_errors.increment(1),
        }
    }
}

pub(crate) fn update_headers_metrics(n_headers: u64) {
    counter!("stages.headers.counter", n_headers);
}

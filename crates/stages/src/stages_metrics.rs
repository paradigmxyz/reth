use metrics::{counter, histogram, increment_counter};
use reth_interfaces::p2p::error::DownloadError;

pub(crate) fn update_headers_error_metrics(error: DownloadError) {
    match error {
        DownloadError::Timeout => increment_counter!("stages.headers.timeout_errors"),
        DownloadError::HeaderValidation { hash, error } => {
            increment_counter!("stages.headers.validation_errors")
        }
        _error => increment_counter!("stages.headers.unexpected_errors"),
    }
}

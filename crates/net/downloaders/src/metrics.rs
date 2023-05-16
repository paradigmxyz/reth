use metrics::{Counter, Gauge};
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
    /// The number of requests (can contain more than 1 item) currently in-flight.
    pub in_flight_requests: Gauge,
    /// The number of responses (can contain more than 1 item) in the internal buffer of the
    /// downloader.
    pub buffered_responses: Gauge,
    /// The number of blocks the internal buffer of the
    /// downloader.
    /// These are bodies that have been received, but not cannot be committed yet because they're
    /// not contiguous
    pub buffered_blocks: Gauge,
    /// The number blocks that are contiguous and are queued for insertion into the db.
    pub queued_blocks: Gauge,
    /// The number of out-of-order requests sent by the downloader.
    /// The consumer of the download stream is able to re-request data (headers or bodies) in case
    /// it encountered a recoverable error (e.g. during insertion).
    /// Out-of-order request happen when:
    ///     - the headers downloader `SyncTarget::Tip` hash is different from the previous sync
    ///       target hash.
    ///     - the new download range start for bodies donwloader is less than the last block number
    ///       returned from the stream.
    pub out_of_order_requests: Counter,
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

use reth_interfaces::p2p::error::DownloadError;
use reth_metrics::{
    metrics::{self, Counter, Gauge},
    Metrics,
};

/// Common body downloader metrics.
///
/// These metrics will be initialized with the `downloaders.bodies` scope.
/// ```
/// use reth_downloaders::metrics::BodyDownloaderMetrics;
/// use reth_interfaces::p2p::error::DownloadError;
///
/// // Initialize metrics.
/// let metrics = BodyDownloaderMetrics::default();
/// // Increment `downloaders.bodies.timeout_errors` counter by 1.
/// metrics.increment_errors(&DownloadError::Timeout);
/// ```
#[derive(Clone, Metrics)]
#[metrics(scope = "downloaders.bodies")]
pub struct BodyDownloaderMetrics {
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
    /// Total amount of memory used by the buffered blocks in bytes
    pub buffered_blocks_size_bytes: Gauge,
    /// The number blocks that are contiguous and are queued for insertion into the db.
    pub queued_blocks: Gauge,
    /// The number of out-of-order requests sent by the downloader.
    /// The consumer of the download stream is able to re-request data (bodies) in case
    /// it encountered a recoverable error (e.g. during insertion).
    /// Out-of-order request happen when the new download range start for bodies downloader
    /// is less than the last block number returned from the stream.
    pub out_of_order_requests: Counter,
    /// Number of timeout errors while requesting items
    pub timeout_errors: Counter,
    /// Number of validation errors while requesting items
    pub validation_errors: Counter,
    /// Number of unexpected errors while requesting items
    pub unexpected_errors: Counter,
}

impl BodyDownloaderMetrics {
    /// Increment errors counter.
    pub fn increment_errors(&self, error: &DownloadError) {
        match error {
            DownloadError::Timeout => self.timeout_errors.increment(1),
            DownloadError::BodyValidation { .. } => self.validation_errors.increment(1),
            _error => self.unexpected_errors.increment(1),
        }
    }
}

/// Metrics for an individual response, i.e. the size in bytes, and length (number of bodies) in the
/// response.
///
/// These metrics will be initialized with the `downloaders.bodies.response` scope.
#[derive(Clone, Metrics)]
#[metrics(scope = "downloaders.bodies.response")]
pub struct ResponseMetrics {
    /// The size (in bytes) of an individual bodies response received by the downloader.
    pub response_size_bytes: Gauge,
    /// The number of bodies in an individual bodies response received by the downloader.
    pub response_length: Gauge,
}

/// Common header downloader metrics.
///
/// These metrics will be initialized with the `downloaders.headers` scope.
/// ```
/// use reth_downloaders::metrics::HeaderDownloaderMetrics;
/// use reth_interfaces::p2p::error::DownloadError;
///
/// // Initialize metrics.
/// let metrics = HeaderDownloaderMetrics::default();
/// // Increment `downloaders.headers.timeout_errors` counter by 1.
/// metrics.increment_errors(&DownloadError::Timeout);
/// ```
#[derive(Clone, Metrics)]
#[metrics(scope = "downloaders.headers")]
pub struct HeaderDownloaderMetrics {
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
    /// Total amount of memory used by the buffered blocks in bytes
    pub buffered_blocks_size_bytes: Gauge,
    /// The number blocks that are contiguous and are queued for insertion into the db.
    pub queued_blocks: Gauge,
    /// The number of out-of-order requests sent by the downloader.
    /// The consumer of the download stream is able to re-request data (headers) in case
    /// it encountered a recoverable error (e.g. during insertion).
    /// Out-of-order request happen when the headers downloader `SyncTarget::Tip`
    /// hash is different from the previous sync target hash.
    pub out_of_order_requests: Counter,
    /// Number of timeout errors while requesting items
    pub timeout_errors: Counter,
    /// Number of validation errors while requesting items
    pub validation_errors: Counter,
    /// Number of unexpected errors while requesting items
    pub unexpected_errors: Counter,
}

impl HeaderDownloaderMetrics {
    /// Increment errors counter.
    pub fn increment_errors(&self, error: &DownloadError) {
        match error {
            DownloadError::Timeout => self.timeout_errors.increment(1),
            DownloadError::HeaderValidation { .. } => self.validation_errors.increment(1),
            _error => self.unexpected_errors.increment(1),
        }
    }
}

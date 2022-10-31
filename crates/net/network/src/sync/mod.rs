//! Network syncing primitives.

mod traits;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
pub use traits::StateSync;

/// Manages syncing operations.
///
/// Core implementation of [`StateSync`].
pub struct StateSyncer {
    inflight_downloads: (),

    /// Receiver for new incoming download requests
    download_requests_rx: ReceiverStream<DownloadRequest>,
    /// Sender for download requests, used to detach a [`HeadersDownloader`]
    download_requests_tx: mpsc::Sender<DownloadRequest>,
}

// === impl StateSyncer ===

impl StateSyncer {
    /// Returns a new [`HeadersDownloader`] that can send requests to this type
    pub fn headers_downloader(&self) -> HeadersDownloader {
        HeadersDownloader { request_tx: self.download_requests_tx.clone() }
    }
}

/// Front-end API for downloading headers.
pub struct HeadersDownloader {
    /// Sender half of the request channel.
    request_tx: mpsc::Sender<DownloadRequest>,
}

// TODO impl Downloader for HeadersDownloader

// === impl HeadersDownloader ===

impl HeadersDownloader {}

/// Requests that can be sent to the Syncer from a [`HeadersDownloader`]
enum DownloadRequest {}

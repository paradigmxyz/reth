use futures::{Future, FutureExt, StreamExt};
use reth_interfaces::{
    consensus::Consensus,
    p2p::{bodies::client::BodiesClient, headers::client::HeadersClient},
};
use reth_primitives::H256;
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::*;

/// Downloader config.
pub mod config;
use config::{BatchDownloadConfig, DownloaderConfig};

/// Downloader error.
pub mod error;
pub use error::{DownloaderError, DownloaderResult};

/// Downloader incoming message.
pub mod message;
use message::DownloaderMessage;

mod download;
use download::{bodies::BodiesDownload, headers::HeadersDownload, Download, DownloadOrigin};

mod buffer;
use buffer::DownloadBuffer;

mod state;
use state::DownloaderState;

mod utils;
pub use utils::{BoundedBinaryHeap, BoundedVec};

/// Optimistic downloader implementation.
#[derive(Debug)]
pub struct Downloader<N, C> {
    /// The network client
    client: Arc<N>,
    /// The consensus client
    consensus: Arc<C>,
    /// Chain tip
    tip: H256, // TODO: move to state
    /// Header batch size.
    config: BatchDownloadConfig,
    /// The buffer of download results.
    buffer: DownloadBuffer,
    /// The request receiver.
    message_rx: UnboundedReceiverStream<DownloaderMessage>,
    /// Pending downloads.
    downloads: BoundedVec<Download<N, C>>,
    /// The download queue.
    queue: BoundedBinaryHeap<Download<N, C>>,
}

impl<N, C> Downloader<N, C>
where
    N: HeadersClient + 'static,
    C: Consensus,
{
    /// Create new instance of downloader.
    pub fn new(
        client: Arc<N>,
        consensus: Arc<C>,
        config: DownloaderConfig,
        request_rx: UnboundedReceiverStream<DownloaderMessage>,
    ) -> Self {
        Self {
            client,
            consensus,
            message_rx: request_rx,
            buffer: DownloadBuffer::new(config.buffer_size),
            config: config.batch,
            tip: Default::default(),
            downloads: BoundedVec::new(config.concurrency),
            queue: BoundedBinaryHeap::new(config.queue_size),
        }
    }

    // Handle incoming download message.
    fn on_message(&mut self, req: DownloaderMessage) {
        match req {
            DownloaderMessage::Headers(tx, msg) => {
                let origin = DownloadOrigin::Remote(tx);

                // First check the buffer
                let mut buffered =
                    self.buffer.retrieve_header_chain(msg.tip, msg.head.hash()).unwrap_or_default();
                let download_tip = buffered.last().map(|h| h.hash()).unwrap_or(msg.tip);
                if download_tip == msg.head.hash() {
                    // Successfully retrieved all requested headers from cache
                    buffered.pop();
                    let _ = buffered.into_iter().try_for_each(|h| origin.try_send_response(h));
                    return
                }

                // Otherwise send retrieved buffers if any and proceed to download with earliest tip
                let _ = buffered.into_iter().try_for_each(|h| origin.try_send_response(h));
                let download = Download::Headers(
                    origin.clone(),
                    HeadersDownload::new(
                        Arc::clone(&self.client),
                        Arc::clone(&self.consensus),
                        download_tip,
                        msg.head,
                        self.config.headers_batch_size,
                    ),
                );
                if self.try_schedule_download(download).is_err() {
                    // Send error. Ignore if receiver was dropped
                    let _ = origin.try_send_error(DownloaderError::Full);
                }
            }
            DownloaderMessage::Bodies(tx, msg) => {
                let origin = DownloadOrigin::Remote(tx);

                let mut download_headers = Vec::with_capacity(msg.len());
                for hash in msg {
                    // Attempt to look up body in buffer
                    if let Some(body) = self.buffer.retrieve_body(hash) {
                        let _ = origin.try_send_response(body);
                    } else
                    // Attempt to look up header in buffer (needed for consensus validation)
                    if let Some(header) = self.buffer.retrieve_header(hash) {
                        if header.is_empty() {
                        } else {
                            download_headers.push(header);
                        }
                    }
                    // TODO: attempt db lookup
                    // Send error back to the requester
                    else {
                        let _ = origin.try_send_error(DownloaderError::MissingHeader(hash));
                    }
                }

                let download = Download::Bodies(
                    origin.clone(),
                    BodiesDownload::new(Arc::clone(&self.client), Arc::clone(&self.consensus)),
                );
                if self.try_schedule_download(download).is_err() {
                    // Send error. Ignore if receiver was dropped
                    let _ = origin.try_send_error(DownloaderError::Full);
                }
            }
        }
    }

    fn try_schedule_download(&mut self, download: Download<N, C>) -> Result<(), Download<N, C>> {
        if let Err(download) = self.downloads.push_within_capacity(download) {
            self.queue.push_within_capacity(download)
        } else {
            Ok(())
        }
    }
}

impl<N, C> Future for Downloader<N, C>
where
    N: HeadersClient + BodiesClient + 'static,
    C: Consensus,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // Process incoming messages
        loop {
            match this.message_rx.poll_next_unpin(cx) {
                Poll::Ready(Some(msg)) => this.on_message(msg),
                Poll::Ready(None) => {
                    // This is only possible if the channel was deliberately closed
                    error!(target: "downloader", "message channel closed");
                    return Poll::Ready(())
                }
                Poll::Pending => break,
            };
        }

        // Poll all pending downloads
        for download in this.downloads.iter_mut() {
            match download {
                Download::Headers(origin, inner) => {
                    if let Poll::Ready(headers) = inner.poll_unpin(cx) {
                        this.buffer.extend_headers(headers.iter().cloned());
                        // Attempt to send responses.
                        // If the receiver channel was dropped, drop the download.
                        let res = headers.into_iter().try_for_each(|h| origin.try_send_response(h));
                        if res.is_err() {
                            error!(target: "downloader", "message receiver dropped");
                            inner.clear();
                        }
                    }
                }
                Download::Bodies(origin, inner) => {
                    if let Poll::Ready(bodies) = inner.poll_unpin(cx) {
                        this.buffer.extend_bodies(bodies.iter().cloned());
                        // Attempt to send responses.
                        // If the receiver channel was dropped, drop the download.
                        let res = bodies.into_iter().try_for_each(|b| origin.try_send_response(b));
                        if res.is_err() {
                            error!(target: "downloader", "message receiver dropped");
                            inner.clear();
                        }
                    }
                }
            }
        }

        // Remove downloads without pending futures
        this.downloads.retain(|d| d.in_progress());

        // Choose next downloads from the queue
        while !this.downloads.at_capacity() {
            match this.queue.pop() {
                Some(download) => this.downloads.push(download),
                None => break,
            }
        }

        // TODO: check at interval? move?
        // Poll the latest chain tip
        let forkchoice_rx = this.consensus.fork_choice_state();
        if forkchoice_rx.has_changed().expect("forkchoice channel dropped") {
            let forkchoice = forkchoice_rx.borrow();
            if !forkchoice.head_block_hash.is_zero() {
                this.tip = forkchoice.head_block_hash;
            }
        }

        // If there are no requests and downloads to process, optimistically initiate downloads
        // TODO:

        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    // TODO:
}

//!
use futures::Stream;
use futures_util::StreamExt;
use pin_project::pin_project;
use reth_interfaces::p2p::headers::downloader::{HeaderDownloader, SyncTarget};
use reth_primitives::SealedHeader;
use reth_rpc_types::Header;
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::{
    sync::{mpsc, mpsc::UnboundedSender},
    task::JoinHandle,
};
use tokio_stream::wrappers::UnboundedReceiverStream;

/// A [HeaderDownloader] that drives a spawned [HeaderDownloader] on a spawned task.
#[pin_project]
pub struct TaskDownloader {
    #[pin]
    from_downloader: UnboundedReceiverStream<Vec<SealedHeader>>,
    to_downloader: UnboundedSender<DownloaderUpdates>,
    _task: JoinHandle<()>,
}

// === impl TaskDownloader ===

impl TaskDownloader {
    /// Spawns the given `downloader` and returns a [TaskDownloader] that's connected to that task.
    pub fn spawn<T: HeaderDownloader>(downloader: T) -> Self {
        let (headers_tx, headers_rx) = mpsc::unbounded_channel();
        let (to_downloader, updates_rx) = mpsc::unbounded_channel();

        let downloader = SpawnedDownloader {
            headers_tx,
            updates: UnboundedReceiverStream::new(updates_rx),
            downloader,
        };

        let _task = tokio::task::spawn(downloader);

        Self { from_downloader: UnboundedReceiverStream::new(headers_rx), to_downloader, _task }
    }
}

impl HeaderDownloader for TaskDownloader {
    fn update_sync_gap(&mut self, head: SealedHeader, target: SyncTarget) {
        let _ = self.to_downloader.send(DownloaderUpdates::UpdateSyncGap(head, target));
    }

    fn update_local_head(&mut self, head: SealedHeader) {
        let _ = self.to_downloader.send(DownloaderUpdates::UpdateLocalHead(head));
    }

    fn update_sync_target(&mut self, target: SyncTarget) {
        let _ = self.to_downloader.send(DownloaderUpdates::UpdateSyncTarget(target));
    }

    fn set_batch_size(&mut self, limit: usize) {
        let _ = self.to_downloader.send(DownloaderUpdates::SetBatchSize(limit));
    }
}

impl Stream for TaskDownloader {
    type Item = Vec<SealedHeader>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.project().from_downloader.poll_next(cx)
    }
}

/// A [HeaderDownloader]
struct SpawnedDownloader<T> {
    updates: UnboundedReceiverStream<DownloaderUpdates>,
    headers_tx: UnboundedSender<Vec<SealedHeader>>,
    downloader: T,
}

impl<T: HeaderDownloader> Future for SpawnedDownloader<T> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            while let Poll::Ready(Some(update)) = this.updates.poll_next_unpin(cx) {
                match update {
                    DownloaderUpdates::UpdateSyncGap(head, target) => {
                        this.downloader.update_sync_gap(head, target);
                    }
                    DownloaderUpdates::UpdateLocalHead(head) => {
                        this.downloader.update_local_head(head);
                    }
                    DownloaderUpdates::UpdateSyncTarget(target) => {
                        this.downloader.update_sync_target(target);
                    }
                    DownloaderUpdates::SetBatchSize(limit) => {
                        this.downloader.set_batch_size(head, limit);
                    }
                }
            }

            if let Some(headers) = ready!(this.downloader.poll_next_unpin(cx)) {
                let _ = this.headers_tx.send(headers);
            }
        }
    }
}

/// Commands delegated tot the spawned [HeaderDownloader]
enum DownloaderUpdates {
    UpdateSyncGap(SealedHeader, SyncTarget),
    UpdateLocalHead(SealedHeader),
    UpdateSyncTarget(SyncTarget),
    SetBatchSize(usize),
}

use futures::Stream;
use futures_util::StreamExt;
use pin_project::pin_project;
use reth_interfaces::p2p::headers::downloader::{HeaderDownloader, SyncTarget};
use reth_primitives::SealedHeader;
use std::{
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::{
    sync::{mpsc, mpsc::UnboundedSender},
    task::JoinSet,
};
use tokio_stream::wrappers::UnboundedReceiverStream;

/// A [HeaderDownloader] that drives a spawned [HeaderDownloader] on a spawned task.
#[derive(Debug)]
#[pin_project]
pub struct TaskDownloader {
    #[pin]
    from_downloader: UnboundedReceiverStream<Vec<SealedHeader>>,
    to_downloader: UnboundedSender<DownloaderUpdates>,
    /// The spawned downloader tasks.
    ///
    /// Note: If this type is dropped, the downloader task gets dropped as well.
    _task: JoinSet<()>,
}

// === impl TaskDownloader ===

impl TaskDownloader {
    /// Spawns the given `downloader` and returns a [TaskDownloader] that's connected to that task.
    ///
    /// # Panics
    ///
    /// This method panics if called outside of a Tokio runtime
    ///
    /// # Example
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use reth_downloaders::headers::linear::LinearDownloader;
    /// # use reth_downloaders::headers::task::TaskDownloader;
    /// # use reth_interfaces::consensus::Consensus;
    /// # use reth_interfaces::p2p::headers::client::HeadersClient;
    /// # fn t<H: HeadersClient + 'static>(consensus:Arc<dyn Consensus>, client: Arc<H>) {
    ///    let downloader = LinearDownloader::<H>::builder().build(
    ///        consensus,
    ///        client,
    ///        Default::default(),
    ///        Default::default(),
    ///    );
    ///   let downloader = TaskDownloader::spawn(downloader);
    /// # }
    pub fn spawn<T>(downloader: T) -> Self
    where
        T: HeaderDownloader + 'static,
    {
        let (headers_tx, headers_rx) = mpsc::unbounded_channel();
        let (to_downloader, updates_rx) = mpsc::unbounded_channel();

        let downloader = SpawnedDownloader {
            headers_tx,
            updates: UnboundedReceiverStream::new(updates_rx),
            downloader,
        };

        let mut task = JoinSet::<()>::new();
        task.spawn(downloader);

        Self {
            from_downloader: UnboundedReceiverStream::new(headers_rx),
            to_downloader,
            _task: task,
        }
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

/// A [HeaderDownloader] that runs on its own task
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
                        this.downloader.set_batch_size(limit);
                    }
                }
            }

            match ready!(this.downloader.poll_next_unpin(cx)) {
                Some(headers) => {
                    let _ = this.headers_tx.send(headers);
                }
                None => return Poll::Pending,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headers::{linear::LinearDownloadBuilder, test_utils::child_header};
    use reth_interfaces::test_utils::{TestConsensus, TestHeadersClient};
    use std::sync::Arc;

    #[tokio::test(flavor = "multi_thread")]
    async fn download_one_by_one_on_task() {
        reth_tracing::init_test_tracing();

        let p3 = SealedHeader::default();
        let p2 = child_header(&p3);
        let p1 = child_header(&p2);
        let p0 = child_header(&p1);

        let client = Arc::new(TestHeadersClient::default());
        let downloader = LinearDownloadBuilder::default()
            .stream_batch_size(1)
            .request_limit(1)
            .build(Arc::new(TestConsensus::default()), Arc::clone(&client), p3.clone(), p0.hash());

        let mut downloader = TaskDownloader::spawn(downloader);

        client
            .extend(vec![
                p0.as_ref().clone(),
                p1.as_ref().clone(),
                p2.as_ref().clone(),
                p3.as_ref().clone(),
            ])
            .await;

        let headers = downloader.next().await.unwrap();
        assert_eq!(headers, vec![p0]);

        let headers = downloader.next().await.unwrap();
        assert_eq!(headers, vec![p1]);
        let headers = downloader.next().await.unwrap();
        assert_eq!(headers, vec![p2]);
    }
}

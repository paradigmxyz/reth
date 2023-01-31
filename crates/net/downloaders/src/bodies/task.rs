use futures::Stream;
use futures_util::StreamExt;
use pin_project::pin_project;
use reth_interfaces::p2p::{
    bodies::downloader::{BodyDownloader, BodyDownloaderResult},
    error::DownloadResult,
};
use reth_primitives::BlockNumber;
use std::{
    future::Future,
    ops::Range,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::{
    sync::{mpsc, mpsc::UnboundedSender},
    task::JoinSet,
};
use tokio_stream::wrappers::UnboundedReceiverStream;

/// A [BodyDownloader] that drives a spawned [BodyDownloader] on a spawned task.
#[derive(Debug)]
#[pin_project]
pub struct TaskDownloader {
    #[pin]
    from_downloader: UnboundedReceiverStream<BodyDownloaderResult>,
    to_downloader: UnboundedSender<Range<BlockNumber>>,
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
    /// use std::sync::Arc;
    /// use reth_downloaders::bodies::bodies::BodiesDownloaderBuilder;
    /// use reth_downloaders::bodies::task::TaskDownloader;
    /// use reth_interfaces::consensus::Consensus;
    /// use reth_interfaces::p2p::bodies::client::BodiesClient;
    /// use reth_db::database::Database;
    /// fn t<B: BodiesClient + 'static, DB: Database + 'static>(client: Arc<B>, consensus:Arc<dyn Consensus>, db: Arc<DB>) {
    ///     let downloader = BodiesDownloaderBuilder::default().build(
    ///         client,
    ///         consensus,
    ///         db
    ///     );
    ///     let downloader = TaskDownloader::spawn(downloader);
    /// }
    /// ```
    pub fn spawn<T>(downloader: T) -> Self
    where
        T: BodyDownloader + 'static,
    {
        let (bodies_tx, bodies_rx) = mpsc::unbounded_channel();
        let (to_downloader, updates_rx) = mpsc::unbounded_channel();

        let downloader = SpawnedDownloader {
            bodies_tx,
            updates: UnboundedReceiverStream::new(updates_rx),
            downloader,
        };

        let mut task = JoinSet::<()>::new();
        task.spawn(downloader);

        Self {
            from_downloader: UnboundedReceiverStream::new(bodies_rx),
            to_downloader,
            _task: task,
        }
    }
}

impl BodyDownloader for TaskDownloader {
    fn set_download_range(&mut self, range: Range<BlockNumber>) -> DownloadResult<()> {
        let _ = self.to_downloader.send(range);
        Ok(())
    }
}

impl Stream for TaskDownloader {
    type Item = BodyDownloaderResult;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.project().from_downloader.poll_next(cx)
    }
}

/// A [BodyDownloader] that runs on its own task
struct SpawnedDownloader<T> {
    updates: UnboundedReceiverStream<Range<BlockNumber>>,
    bodies_tx: UnboundedSender<BodyDownloaderResult>,
    downloader: T,
}

impl<T: BodyDownloader> Future for SpawnedDownloader<T> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            while let Poll::Ready(Some(range)) = this.updates.poll_next_unpin(cx) {
                if let Err(err) = this.downloader.set_download_range(range) {
                    tracing::error!(target: "downloaders::bodies", ?err, "Failed to set download range");
                    let _ = this.bodies_tx.send(Err(err));
                }
            }

            match ready!(this.downloader.poll_next_unpin(cx)) {
                Some(bodies) => {
                    let _ = this.bodies_tx.send(bodies);
                }
                None => return Poll::Pending,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bodies::{
            bodies::BodiesDownloaderBuilder,
            test_utils::{insert_headers, zip_blocks},
        },
        test_utils::{generate_bodies, TestBodiesClient},
    };
    use assert_matches::assert_matches;
    use reth_db::mdbx::{test_utils::create_test_db, EnvKind, WriteMap};
    use reth_interfaces::{p2p::error::DownloadError, test_utils::TestConsensus};
    use std::sync::Arc;

    #[tokio::test(flavor = "multi_thread")]
    async fn download_one_by_one_on_task() {
        reth_tracing::init_test_tracing();

        let db = create_test_db::<WriteMap>(EnvKind::RW);
        let (headers, mut bodies) = generate_bodies(0..20);

        insert_headers(&db, &headers);

        let client = Arc::new(
            TestBodiesClient::default().with_bodies(bodies.clone()).with_should_delay(true),
        );
        let downloader = BodiesDownloaderBuilder::default().build(
            client.clone(),
            Arc::new(TestConsensus::default()),
            db,
        );
        let mut downloader = TaskDownloader::spawn(downloader);

        downloader.set_download_range(0..20).expect("failed to set download range");

        assert_matches!(
            downloader.next().await,
            Some(Ok(res)) => assert_eq!(res, zip_blocks(headers.iter(), &mut bodies))
        );
        assert_eq!(client.times_requested(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn set_download_range_error_returned() {
        reth_tracing::init_test_tracing();

        let db = create_test_db::<WriteMap>(EnvKind::RW);
        let (headers, mut bodies) = generate_bodies(0..20);

        // Insert a subset of headers to the database
        insert_headers(&db, &headers[10..]);

        let client = Arc::new(
            TestBodiesClient::default().with_bodies(bodies.clone()).with_should_delay(true),
        );
        let downloader = BodiesDownloaderBuilder::default().build(
            client.clone(),
            Arc::new(TestConsensus::default()),
            db,
        );
        let mut downloader = TaskDownloader::spawn(downloader);

        downloader.set_download_range(10..20).expect("failed to set download range");
        assert_matches!(
            downloader.next().await,
            Some(Ok(res)) => assert_eq!(res, zip_blocks(headers.iter().skip(10), &mut bodies))
        );

        downloader.set_download_range(0..20).expect("failed to set download range");
        assert_matches!(
            downloader.next().await,
            Some(Err(DownloadError::MissingHeader { block_number: 0 }))
        );
    }
}

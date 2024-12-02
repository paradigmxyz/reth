use alloy_primitives::BlockNumber;
use futures::Stream;
use futures_util::{FutureExt, StreamExt};
use pin_project::pin_project;
use reth_network_p2p::{
    bodies::downloader::{BodyDownloader, BodyDownloaderResult},
    error::DownloadResult,
};
use reth_tasks::{TaskSpawner, TokioTaskExecutor};
use std::{
    fmt::Debug,
    future::Future,
    ops::RangeInclusive,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::sync::{mpsc, mpsc::UnboundedSender};
use tokio_stream::wrappers::{ReceiverStream, UnboundedReceiverStream};
use tokio_util::sync::PollSender;

/// The maximum number of [`BodyDownloaderResult`]s to hold in the buffer.
pub const BODIES_TASK_BUFFER_SIZE: usize = 4;

/// A [BodyDownloader] that drives a spawned [BodyDownloader] on a spawned task.
#[derive(Debug)]
#[pin_project]
pub struct TaskDownloader<B> {
    #[pin]
    from_downloader: ReceiverStream<BodyDownloaderResult<B>>,
    to_downloader: UnboundedSender<RangeInclusive<BlockNumber>>,
}

// === impl TaskDownloader ===

impl<B: Send + Sync + Unpin + 'static> TaskDownloader<B> {
    /// Spawns the given `downloader` via [`tokio::task::spawn`] returns a [`TaskDownloader`] that's
    /// connected to that task.
    ///
    /// # Panics
    ///
    /// This method panics if called outside of a Tokio runtime
    ///
    /// # Example
    ///
    /// ```
    /// use reth_consensus::Consensus;
    /// use reth_downloaders::bodies::{bodies::BodiesDownloaderBuilder, task::TaskDownloader};
    /// use reth_network_p2p::bodies::client::BodiesClient;
    /// use reth_primitives_traits::InMemorySize;
    /// use reth_storage_api::HeaderProvider;
    /// use std::{fmt::Debug, sync::Arc};
    ///
    /// fn t<
    ///     B: BodiesClient<Body: Debug + InMemorySize> + 'static,
    ///     Provider: HeaderProvider<Header = alloy_consensus::Header> + Unpin + 'static,
    /// >(
    ///     client: Arc<B>,
    ///     consensus: Arc<dyn Consensus<Provider::Header, B::Body>>,
    ///     provider: Provider,
    /// ) {
    ///     let downloader = BodiesDownloaderBuilder::default().build(client, consensus, provider);
    ///     let downloader = TaskDownloader::spawn(downloader);
    /// }
    /// ```
    pub fn spawn<T>(downloader: T) -> Self
    where
        T: BodyDownloader<Body = B> + 'static,
    {
        Self::spawn_with(downloader, &TokioTaskExecutor::default())
    }

    /// Spawns the given `downloader` via the given [`TaskSpawner`] returns a [`TaskDownloader`]
    /// that's connected to that task.
    pub fn spawn_with<T, S>(downloader: T, spawner: &S) -> Self
    where
        T: BodyDownloader<Body = B> + 'static,
        S: TaskSpawner,
    {
        let (bodies_tx, bodies_rx) = mpsc::channel(BODIES_TASK_BUFFER_SIZE);
        let (to_downloader, updates_rx) = mpsc::unbounded_channel();

        let downloader = SpawnedDownloader {
            bodies_tx: PollSender::new(bodies_tx),
            updates: UnboundedReceiverStream::new(updates_rx),
            downloader,
        };

        spawner.spawn(downloader.boxed());

        Self { from_downloader: ReceiverStream::new(bodies_rx), to_downloader }
    }
}

impl<B: Debug + Send + Sync + Unpin + 'static> BodyDownloader for TaskDownloader<B> {
    type Body = B;

    fn set_download_range(&mut self, range: RangeInclusive<BlockNumber>) -> DownloadResult<()> {
        let _ = self.to_downloader.send(range);
        Ok(())
    }
}

impl<B> Stream for TaskDownloader<B> {
    type Item = BodyDownloaderResult<B>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.project().from_downloader.poll_next(cx)
    }
}

/// A [`BodyDownloader`] that runs on its own task
struct SpawnedDownloader<T: BodyDownloader> {
    updates: UnboundedReceiverStream<RangeInclusive<BlockNumber>>,
    bodies_tx: PollSender<BodyDownloaderResult<T::Body>>,
    downloader: T,
}

impl<T: BodyDownloader> Future for SpawnedDownloader<T> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            while let Poll::Ready(update) = this.updates.poll_next_unpin(cx) {
                if let Some(range) = update {
                    if let Err(err) = this.downloader.set_download_range(range) {
                        tracing::error!(target: "downloaders::bodies", %err, "Failed to set bodies download range");

                        // Clone the sender ensure its availability. See [PollSender::clone].
                        let mut bodies_tx = this.bodies_tx.clone();

                        let forward_error_result = ready!(bodies_tx.poll_reserve(cx))
                            .and_then(|_| bodies_tx.send_item(Err(err)));
                        if forward_error_result.is_err() {
                            // channel closed, this means [TaskDownloader] was dropped,
                            // so we can also exit
                            return Poll::Ready(())
                        }
                    }
                } else {
                    // channel closed, this means [TaskDownloader] was dropped, so we can also
                    // exit
                    return Poll::Ready(())
                }
            }

            match ready!(this.bodies_tx.poll_reserve(cx)) {
                Ok(()) => match ready!(this.downloader.poll_next_unpin(cx)) {
                    Some(bodies) => {
                        if this.bodies_tx.send_item(bodies).is_err() {
                            // channel closed, this means [TaskDownloader] was dropped, so we can
                            // also exit
                            return Poll::Ready(())
                        }
                    }
                    None => return Poll::Pending,
                },
                Err(_) => {
                    // channel closed, this means [TaskDownloader] was dropped, so we can also
                    // exit
                    return Poll::Ready(())
                }
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
    use reth_consensus::test_utils::TestConsensus;
    use reth_network_p2p::error::DownloadError;
    use reth_provider::test_utils::create_test_provider_factory;
    use std::sync::Arc;

    #[tokio::test(flavor = "multi_thread")]
    async fn download_one_by_one_on_task() {
        reth_tracing::init_test_tracing();

        let factory = create_test_provider_factory();
        let (headers, mut bodies) = generate_bodies(0..=19);

        insert_headers(factory.db_ref().db(), &headers);

        let client = Arc::new(
            TestBodiesClient::default().with_bodies(bodies.clone()).with_should_delay(true),
        );
        let downloader = BodiesDownloaderBuilder::default().build(
            client.clone(),
            Arc::new(TestConsensus::default()),
            factory,
        );
        let mut downloader = TaskDownloader::spawn(downloader);

        downloader.set_download_range(0..=19).expect("failed to set download range");

        assert_matches!(
            downloader.next().await,
            Some(Ok(res)) => assert_eq!(res, zip_blocks(headers.iter(), &mut bodies))
        );
        assert_eq!(client.times_requested(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[allow(clippy::reversed_empty_ranges)]
    async fn set_download_range_error_returned() {
        reth_tracing::init_test_tracing();
        let factory = create_test_provider_factory();

        let downloader = BodiesDownloaderBuilder::default().build(
            Arc::new(TestBodiesClient::default()),
            Arc::new(TestConsensus::default()),
            factory,
        );
        let mut downloader = TaskDownloader::spawn(downloader);

        downloader.set_download_range(1..=0).expect("failed to set download range");
        assert_matches!(downloader.next().await, Some(Err(DownloadError::InvalidBodyRange { .. })));
    }
}

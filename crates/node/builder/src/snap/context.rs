//! Reports the node's headers, peers and forkchoice movement to a snap bootstrap run.

use alloy_primitives::B256;
use reth_network_p2p::download::DownloadClient;
use reth_provider::{
    providers::ProviderNodeTypes, BlockNumReader, DatabaseProviderFactory, ProviderFactory,
};
use reth_snap_sync::{SnapSyncContext, SnapSyncError};
use std::time::Duration;
use tokio::sync::watch;

// Sampling well below the block time resumes a stalled step soon after new peers connect.
const DEFAULT_SAMPLE_INTERVAL: Duration = Duration::from_secs(2);

/// [`SnapSyncContext`] for one bootstrap run inside the snap backfill.
///
/// Headers only advance between runs, so a new forkchoice target ends a wait and lets the
/// backfill refresh them.
#[derive(Debug)]
pub(crate) struct NodeSnapContext<N: ProviderNodeTypes, C> {
    factory: ProviderFactory<N>,
    // Peer counts come from the client serving the run's requests.
    client: C,
    // Forkchoice targets forwarded by the engine.
    targets: watch::Receiver<B256>,
    interval: Duration,
}

impl<N: ProviderNodeTypes, C> NodeSnapContext<N, C> {
    pub(crate) const fn new(
        factory: ProviderFactory<N>,
        client: C,
        targets: watch::Receiver<B256>,
    ) -> Self {
        Self { factory, client, targets, interval: DEFAULT_SAMPLE_INTERVAL }
    }

    #[cfg(test)]
    const fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }
}

impl<N, C> SnapSyncContext for NodeSnapContext<N, C>
where
    N: ProviderNodeTypes,
    C: DownloadClient + Send + Sync,
{
    fn head(&self) -> Result<u64, SnapSyncError> {
        Ok(self.factory.database_provider_ro()?.last_block_number()?)
    }

    async fn wait_for_progress(&mut self, _head: u64) -> bool {
        // Peers connected before the wait already failed to serve the stalled step.
        let peers = self.client.num_connected_peers();
        loop {
            tokio::select! {
                // New headers need the pipeline, which only runs once this run stops. A closed
                // channel means the backfill is gone.
                _ = self.targets.changed() => return false,
                () = tokio::time::sleep(self.interval) => {}
            }
            if self.client.num_connected_peers() > peers {
                return true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_network_api::PeerId;
    use reth_provider::test_utils::{create_test_provider_factory, MockNodeTypesWithDB};
    use std::sync::atomic::{AtomicUsize, Ordering};

    const TEST_INTERVAL: Duration = Duration::from_millis(5);

    fn context(
        peers: &TestPeers,
    ) -> (watch::Sender<B256>, NodeSnapContext<MockNodeTypesWithDB, &TestPeers>) {
        let (targets, receiver) = watch::channel(B256::repeat_byte(1));
        let context = NodeSnapContext::new(create_test_provider_factory(), peers, receiver)
            .with_interval(TEST_INTERVAL);
        (targets, context)
    }

    #[tokio::test]
    async fn a_new_target_ends_the_wait_for_headers() {
        let peers = TestPeers::default();
        let (targets, mut context) = context(&peers);

        targets.send(B256::repeat_byte(2)).unwrap();

        assert!(!context.wait_for_progress(0).await);
    }

    #[tokio::test]
    async fn a_dropped_backfill_ends_the_wait() {
        let peers = TestPeers::default();
        let (targets, mut context) = context(&peers);

        drop(targets);

        assert!(!context.wait_for_progress(0).await);
    }

    #[tokio::test]
    async fn new_peers_end_the_wait() {
        let peers = TestPeers::default();
        let (_targets, mut context) = context(&peers);

        // Peers count as new only once the wait has sampled its baseline.
        let (progressed, ()) = tokio::join!(context.wait_for_progress(0), async {
            tokio::time::sleep(TEST_INTERVAL).await;
            peers.0.fetch_add(1, Ordering::Relaxed);
        });

        assert!(progressed);
    }

    // A peer count the test moves between samples.
    #[derive(Debug, Default)]
    struct TestPeers(AtomicUsize);

    impl DownloadClient for TestPeers {
        fn report_bad_message(&self, _peer_id: PeerId) {}

        fn num_connected_peers(&self) -> usize {
            self.0.load(Ordering::Relaxed)
        }
    }
}

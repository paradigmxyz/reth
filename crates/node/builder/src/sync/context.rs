//! Reports canonical head and peer progress to a waiting snap sync.
//!
//! Header downloads publish no notification of their own, so progress is sampled from the
//! database and the client's peer count.

use reth_network_p2p::download::DownloadClient;
use reth_provider::{
    providers::ProviderNodeTypes, BlockNumReader, DatabaseProviderFactory, ProviderFactory,
};
use reth_snap_sync::{SnapSyncContext, SnapSyncError};
use std::time::Duration;
use tracing::debug;

// Sampling well below the block time keeps a resumed step close to the head it waited for.
const DEFAULT_SAMPLE_INTERVAL: Duration = Duration::from_secs(2);

/// Samples head and peer progress from the node's database and snap client.
#[derive(Debug)]
pub(crate) struct NodeSnapContext<N: ProviderNodeTypes, C> {
    // Head reads observe the database the sync writes through.
    factory: ProviderFactory<N>,
    // Peer counts come from the client already serving the sync's requests.
    client: C,
    interval: Duration,
}

impl<N: ProviderNodeTypes, C> NodeSnapContext<N, C> {
    pub(crate) const fn new(factory: ProviderFactory<N>, client: C) -> Self {
        Self { factory, client, interval: DEFAULT_SAMPLE_INTERVAL }
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
    C: DownloadClient + Sync,
{
    fn head(&self) -> Result<u64, SnapSyncError> {
        Ok(self.factory.database_provider_ro()?.last_block_number()?)
    }

    // Never gives up on its own: the sync's cancellation ends the wait on shutdown.
    async fn wait_for_progress(&mut self, head: u64) -> bool {
        // Peers connected before the wait already failed to serve the stalled step.
        let peers = self.client.num_connected_peers();
        loop {
            tokio::time::sleep(self.interval).await;
            if self.client.num_connected_peers() > peers {
                return true
            }
            match self.head() {
                Ok(current) if current > head => return true,
                Ok(_) => {}
                Err(error) => debug!(target: "sync::snap", %error, "Failed to read the head"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_primitives::B256;
    use reth_network_api::PeerId;
    use reth_provider::{
        test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
        StaticFileProviderFactory, StaticFileSegment, StaticFileWriter,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    const TEST_INTERVAL: Duration = Duration::from_millis(5);

    fn factory_with_headers(count: u64) -> ProviderFactory<MockNodeTypesWithDB> {
        let factory = create_test_provider_factory();
        let static_files = factory.static_file_provider();
        let mut writer = static_files.latest_writer(StaticFileSegment::Headers).unwrap();
        let mut parent = B256::ZERO;
        for number in 0..count {
            let header = Header { number, parent_hash: parent, ..Default::default() };
            let hash = header.hash_slow();
            writer.append_header(&header, &hash).unwrap();
            parent = hash;
        }
        writer.commit().unwrap();
        drop(writer);
        factory
    }

    #[test]
    fn head_is_the_highest_downloaded_header() {
        let context = NodeSnapContext::new(factory_with_headers(3), TestPeers::default());

        assert_eq!(context.head().unwrap(), 2);
    }

    #[tokio::test]
    async fn an_advanced_head_ends_the_wait() {
        let mut context = NodeSnapContext::new(factory_with_headers(3), TestPeers::default())
            .with_interval(TEST_INTERVAL);

        assert!(context.wait_for_progress(1).await);
    }

    #[tokio::test]
    async fn new_peers_end_the_wait() {
        let peers = TestPeers::default();
        let mut context =
            NodeSnapContext::new(factory_with_headers(1), &peers).with_interval(TEST_INTERVAL);

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

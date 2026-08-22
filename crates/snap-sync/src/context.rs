//! Reports canonical head and peer progress to a stalled Snap session.
//!
//! Header downloads publish no notification of their own, so progress is sampled from the local
//! database and the Snap client's peer count between shutdown-aware waits.

use crate::{error::db_error, SnapSyncContext, SnapSyncError};
use core::time::Duration;
use reth_network_p2p::download::DownloadClient;
use reth_provider::DatabaseProviderFactory;
use reth_storage_api::BlockNumReader;
use reth_tasks::shutdown::Shutdown;
use tracing::debug;

// Sampling well below the block time keeps a resumed phase close to the head it waited for.
const DEFAULT_SAMPLE_INTERVAL: Duration = Duration::from_secs(2);

/// Observes head and peer progress through the node's provider factory and Snap client.
#[derive(Debug)]
pub struct NodeSnapContext<'a, F, C> {
    // Head reads observe the same factory the session writes through.
    factory: &'a F,
    // Peer counts come from the client already serving the session's requests.
    client: &'a C,
    // A fired shutdown ends the session instead of the current wait.
    shutdown: Shutdown,
    // Interval between head and peer samples.
    interval: Duration,
}

impl<'a, F, C> NodeSnapContext<'a, F, C> {
    /// Creates a context whose waits end when `shutdown` fires.
    pub const fn new(factory: &'a F, client: &'a C, shutdown: Shutdown) -> Self {
        Self { factory, client, shutdown, interval: DEFAULT_SAMPLE_INTERVAL }
    }

    /// Sets how often head and peer progress is sampled while waiting.
    pub const fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }
}

impl<F, C> SnapSyncContext for NodeSnapContext<'_, F, C>
where
    F: DatabaseProviderFactory<Provider: BlockNumReader>,
    C: DownloadClient,
{
    fn canonical_head(&self) -> Result<u64, SnapSyncError> {
        let provider = self.factory.database_provider_ro().map_err(db_error)?;
        provider.last_block_number().map_err(db_error)
    }

    async fn wait_for_progress(&mut self, head: u64) -> bool {
        // Peers connected before the wait already failed to serve the stalled phase.
        let peers = self.client.num_connected_peers();
        loop {
            if tokio::time::timeout(self.interval, self.shutdown.clone()).await.is_ok() {
                return false
            }
            if self.client.num_connected_peers() > peers {
                return true
            }
            match self.canonical_head() {
                Ok(current) if current > head => return true,
                Ok(_) => {}
                Err(error) => {
                    debug!(target: "snap::session", %error, "Failed to read the canonical head");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_primitives::B256;
    use reth_network_peers::PeerId;
    use reth_provider::{
        test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
        ProviderFactory, StaticFileProviderFactory, StaticFileWriter,
    };
    use reth_static_file_types::StaticFileSegment;
    use reth_tasks::shutdown::signal;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Sampling faster than the default keeps the waiting tests short.
    const TEST_INTERVAL: Duration = Duration::from_millis(5);

    fn headers(factory: &ProviderFactory<MockNodeTypesWithDB>, count: u64) {
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
        drop(static_files);
    }

    #[tokio::test]
    async fn head_is_the_highest_canonical_header() {
        let factory = create_test_provider_factory();
        headers(&factory, 3);
        let client = TestPeers::new(0);
        let (_signal, shutdown) = signal();

        let context = NodeSnapContext::new(&factory, &client, shutdown);

        assert_eq!(context.canonical_head().unwrap(), 2);
    }

    #[tokio::test]
    async fn advanced_head_ends_the_wait() {
        let factory = create_test_provider_factory();
        headers(&factory, 3);
        let client = TestPeers::new(0);
        let (_signal, shutdown) = signal();
        let mut context =
            NodeSnapContext::new(&factory, &client, shutdown).with_interval(TEST_INTERVAL);

        assert!(context.wait_for_progress(1).await);
    }

    #[tokio::test]
    async fn new_peers_end_the_wait() {
        let factory = create_test_provider_factory();
        headers(&factory, 1);
        let client = TestPeers::new(1);
        let (_signal, shutdown) = signal();
        let mut context =
            NodeSnapContext::new(&factory, &client, shutdown).with_interval(TEST_INTERVAL);

        // Peers must arrive after the wait sampled its baseline to count as new.
        let (progressed, ()) = tokio::join!(context.wait_for_progress(0), async {
            tokio::time::sleep(TEST_INTERVAL).await;
            client.connect(1);
        });

        assert!(progressed);
    }

    #[tokio::test]
    async fn shutdown_ends_the_session() {
        let factory = create_test_provider_factory();
        headers(&factory, 1);
        let client = TestPeers::new(1);
        let (signal, shutdown) = signal();
        let mut context =
            NodeSnapContext::new(&factory, &client, shutdown).with_interval(TEST_INTERVAL);
        signal.fire();

        assert!(!context.wait_for_progress(0).await);
    }

    // A peer count the test moves between samples.
    #[derive(Debug)]
    struct TestPeers(AtomicUsize);

    impl TestPeers {
        fn new(peers: usize) -> Self {
            Self(AtomicUsize::new(peers))
        }

        fn connect(&self, peers: usize) {
            self.0.fetch_add(peers, Ordering::Relaxed);
        }
    }

    impl DownloadClient for TestPeers {
        fn report_bad_message(&self, _peer_id: PeerId) {}

        fn num_connected_peers(&self) -> usize {
            self.0.load(Ordering::Relaxed)
        }
    }
}

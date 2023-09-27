use reth_interfaces::p2p::{
    bodies::client::{BodiesClient, BodiesFut},
    download::DownloadClient,
    priority::Priority,
};
use reth_primitives::{BlockBody, PeerId, B256};
use std::{
    collections::HashMap,
    fmt::Debug,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::Mutex;

/// A [BodiesClient] for testing.
#[derive(Debug, Default)]
pub struct TestBodiesClient {
    bodies: Arc<Mutex<HashMap<B256, BlockBody>>>,
    should_delay: bool,
    max_batch_size: Option<usize>,
    times_requested: AtomicU64,
}

impl TestBodiesClient {
    pub(crate) fn with_bodies(mut self, bodies: HashMap<B256, BlockBody>) -> Self {
        self.bodies = Arc::new(Mutex::new(bodies));
        self
    }

    pub(crate) fn with_should_delay(mut self, should_delay: bool) -> Self {
        self.should_delay = should_delay;
        self
    }

    pub(crate) fn with_max_batch_size(mut self, max_batch_size: usize) -> Self {
        self.max_batch_size = Some(max_batch_size);
        self
    }

    pub(crate) fn times_requested(&self) -> u64 {
        self.times_requested.load(Ordering::Relaxed)
    }
}

impl DownloadClient for TestBodiesClient {
    fn report_bad_message(&self, _peer_id: PeerId) {
        // noop
    }

    fn num_connected_peers(&self) -> usize {
        0
    }
}

impl BodiesClient for TestBodiesClient {
    type Output = BodiesFut;

    fn get_block_bodies_with_priority(
        &self,
        hashes: Vec<B256>,
        _priority: Priority,
    ) -> Self::Output {
        let should_delay = self.should_delay;
        let bodies = self.bodies.clone();
        let max_batch_size = self.max_batch_size;

        self.times_requested.fetch_add(1, Ordering::Relaxed);

        Box::pin(async move {
            if should_delay {
                tokio::time::sleep(Duration::from_millis((hashes[0][0] % 100) as u64)).await;
            }

            let bodies = &mut *bodies.lock().await;
            Ok((
                PeerId::default(),
                hashes
                    .into_iter()
                    .take(max_batch_size.unwrap_or(usize::MAX))
                    .map(|hash| {
                        bodies
                            .remove(&hash)
                            .expect("Downloader asked for a block it should not ask for")
                    })
                    .collect(),
            )
                .into())
        })
    }
}

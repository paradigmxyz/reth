//! Best-effort fetching of historical EIP-7928 block access lists.

use alloy_consensus::BlockHeader;
use alloy_eip7928::bal::{DecodedBal, RawBal};
use alloy_eips::NumHash;
use alloy_primitives::B256;
use futures_util::{stream::FuturesUnordered, Stream, StreamExt};
use reth_config::config::BlockAccessListsConfig;
use reth_metrics::{metrics::Counter, Metrics};
use reth_network_p2p::{
    block_access_lists::client::{BalRequirement, BlockAccessListsClient},
    bodies::{
        downloader::{BodyDownloader, BodyDownloaderResult},
        response::BlockResponse,
    },
};
use reth_primitives_traits::{Block, BlockBody};
use reth_storage_api::BalStoreHandle;
use reth_tasks::TaskExecutor;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::mpsc::{self, error::TrySendError};
use tracing::debug;

/// A body downloader wrapper that schedules optional block access-list downloads.
#[derive(Debug)]
pub struct BalPrefetchingBodiesDownloader<D> {
    inner: D,
    prefetcher: Option<HistoricalBalPrefetcher>,
}

impl<D> BalPrefetchingBodiesDownloader<D> {
    /// Creates a body downloader wrapper.
    pub const fn new(inner: D, prefetcher: Option<HistoricalBalPrefetcher>) -> Self {
        Self { inner, prefetcher }
    }
}

impl<D> Stream for BalPrefetchingBodiesDownloader<D>
where
    D: BodyDownloader,
{
    type Item = BodyDownloaderResult<D::Block>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(responses))) => {
                if let Some(prefetcher) = &this.prefetcher {
                    prefetcher.enqueue_responses(&responses);
                }
                Poll::Ready(Some(Ok(responses)))
            }
            poll => poll,
        }
    }
}

impl<D> BodyDownloader for BalPrefetchingBodiesDownloader<D>
where
    D: BodyDownloader,
{
    type Block = D::Block;

    fn set_download_range(
        &mut self,
        range: std::ops::RangeInclusive<alloy_primitives::BlockNumber>,
    ) -> reth_network_p2p::error::DownloadResult<()> {
        self.inner.set_download_range(range)
    }
}

/// Handle used by the body downloader to enqueue historical BAL candidates without blocking.
#[derive(Clone, Debug)]
pub struct HistoricalBalPrefetcher {
    candidates: mpsc::Sender<HistoricalBalCandidate>,
    metrics: HistoricalBalFetchMetrics,
}

impl HistoricalBalPrefetcher {
    /// Starts a best-effort BAL fetching task when fetching is enabled.
    pub fn spawn<C>(
        config: BlockAccessListsConfig,
        client: C,
        bal_store: BalStoreHandle,
        task_executor: &TaskExecutor,
    ) -> Option<Self>
    where
        C: BlockAccessListsClient + Clone + Send + Sync + 'static,
    {
        if !config.downloader_enabled {
            return None
        }

        let metrics = HistoricalBalFetchMetrics::default();
        let (candidates, receiver) =
            mpsc::channel(config.downloader_max_buffered_candidates.max(1));
        task_executor.spawn_task(run_prefetcher(
            receiver,
            client,
            bal_store,
            config,
            metrics.clone(),
        ));

        Some(Self { candidates, metrics })
    }

    /// Enqueues candidates from downloaded body responses.
    pub fn enqueue_responses<B: Block>(&self, responses: &[BlockResponse<B>]) {
        for response in responses {
            let Some(candidate) = HistoricalBalCandidate::from_response(response) else { continue };
            match self.candidates.try_send(candidate) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => self.metrics.skipped.increment(1),
                // The task is shut down. The body downloader must continue normally.
                Err(TrySendError::Closed(_)) => self.metrics.unavailable.increment(1),
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct HistoricalBalCandidate {
    block: NumHash,
    expected_hash: B256,
    transaction_count: usize,
    gas_used: u64,
}

impl HistoricalBalCandidate {
    fn from_response<B: Block>(response: &BlockResponse<B>) -> Option<Self> {
        let (header, block_hash, transaction_count) = match response {
            BlockResponse::Full(block) => {
                (block.header(), block.hash(), block.body().transactions().len())
            }
            BlockResponse::Empty(header) => (header.header(), header.hash(), 0),
        };
        let expected_hash = header.block_access_list_hash()?;

        Some(Self {
            block: NumHash::new(header.number(), block_hash),
            expected_hash,
            transaction_count,
            gas_used: header.gas_used(),
        })
    }

    const fn is_eligible(self, config: BlockAccessListsConfig) -> bool {
        self.transaction_count >= config.downloader_min_transaction_count &&
            self.gas_used >= config.downloader_min_gas_used
    }
}

async fn run_prefetcher<C>(
    mut receiver: mpsc::Receiver<HistoricalBalCandidate>,
    client: C,
    bal_store: BalStoreHandle,
    config: BlockAccessListsConfig,
    metrics: HistoricalBalFetchMetrics,
) where
    C: BlockAccessListsClient + Clone + Send + Sync + 'static,
{
    let max_concurrent_requests = config.downloader_max_concurrent_requests.max(1);
    let request_limit = config.downloader_request_limit.max(1);
    let mut inflight = FuturesUnordered::new();

    while let Some(candidate) = receiver.recv().await {
        let mut batch = vec![candidate];
        while batch.len() < request_limit {
            match receiver.try_recv() {
                Ok(candidate) => batch.push(candidate),
                Err(_) => break,
            }
        }

        if inflight.len() >= max_concurrent_requests {
            let _ = inflight.next().await;
        }

        let client = client.clone();
        let bal_store = bal_store.clone();
        let metrics = metrics.clone();
        inflight.push(async move {
            fetch_batch(client, bal_store, config, metrics, batch).await;
        });
    }

    while inflight.next().await.is_some() {}
}

async fn fetch_batch<C>(
    client: C,
    bal_store: BalStoreHandle,
    config: BlockAccessListsConfig,
    metrics: HistoricalBalFetchMetrics,
    candidates: Vec<HistoricalBalCandidate>,
) where
    C: BlockAccessListsClient,
{
    let candidate_count = candidates.len();
    let eligible = candidates
        .into_iter()
        .filter(|candidate| candidate.is_eligible(config))
        .collect::<Vec<_>>();
    metrics.skipped.increment((candidate_count - eligible.len()) as u64);
    if eligible.is_empty() {
        return
    }

    let hashes = eligible.iter().map(|candidate| candidate.block.hash).collect::<Vec<_>>();
    let existing = match bal_store.get_by_hashes(&hashes) {
        Ok(existing) => existing,
        Err(err) => {
            metrics.unavailable.increment(eligible.len() as u64);
            debug!(target: "sync::bal_fetch", %err, "Failed to query block access-list store");
            return
        }
    };

    let missing = eligible
        .into_iter()
        .zip(existing)
        .filter_map(|(candidate, existing)| {
            if existing.is_some() {
                metrics.skipped.increment(1);
                None
            } else {
                Some(candidate)
            }
        })
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return
    }

    metrics.requested.increment(missing.len() as u64);
    let hashes = missing.iter().map(|candidate| candidate.block.hash).collect::<Vec<_>>();
    let response = match client
        .get_block_access_lists_with_requirement(hashes, BalRequirement::Optional)
        .await
    {
        Ok(response) => response,
        Err(err) => {
            metrics.unavailable.increment(missing.len() as u64);
            debug!(target: "sync::bal_fetch", %err, "Block access-list request failed");
            return
        }
    };

    let (peer, response) = response.split();
    if response.0.len() > missing.len() {
        metrics.invalid.increment(response.0.len() as u64);
        client.report_bad_message(peer);
        return
    }

    let mut entries = Vec::new();
    let mut blocks = Vec::new();
    let mut response = response.0.into_iter();
    for candidate in missing {
        let Some(raw) = response.next() else {
            metrics.unavailable.increment(1);
            continue
        };
        let Some(raw) = raw else {
            metrics.unavailable.increment(1);
            continue
        };

        let raw = RawBal::new(raw);
        if DecodedBal::from_raw_bal(raw.clone()).is_err() || raw.hash() != candidate.expected_hash {
            metrics.invalid.increment(1);
            client.report_bad_message(peer);
            metrics.unavailable.increment(response.count() as u64);
            break
        }

        blocks.push(candidate.block);
        entries.push((candidate.block, raw));
    }

    let downloaded = entries.len() as u64;
    if downloaded == 0 {
        return
    }
    if let Err(err) = bal_store.insert_many(entries).and_then(|()| bal_store.flush(&blocks)) {
        metrics.unavailable.increment(downloaded);
        debug!(target: "sync::bal_fetch", %err, "Failed to persist downloaded block access lists");
        return
    }
    metrics.downloaded.increment(downloaded);
}

/// Metrics for historical BAL fetching.
#[derive(Clone, Metrics)]
#[metrics(scope = "sync.bal_fetch")]
struct HistoricalBalFetchMetrics {
    /// Eligible BALs requested from peers.
    requested: Counter,
    /// Valid BALs persisted in the local store.
    downloaded: Counter,
    /// Candidates skipped due to policy, a local store hit, or queue pressure.
    skipped: Counter,
    /// BALs unavailable from peers or local infrastructure.
    unavailable: Counter,
    /// Malformed or hash-mismatched BALs.
    invalid: Counter,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{keccak256, Bytes};
    use reth_network_p2p::test_utils::TestFullBlockClient;
    use reth_storage_api::{errors::provider::ProviderResult, BalNotificationStream, BalStore};
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    #[derive(Clone, Default)]
    struct TestBalStore(Arc<Mutex<HashMap<B256, Bytes>>>);

    impl BalStore for TestBalStore {
        fn insert(&self, block: NumHash, bal: RawBal) -> ProviderResult<()> {
            self.0.lock().unwrap().insert(block.hash, bal.into_raw());
            Ok(())
        }

        fn prune(&self, _tip: alloy_primitives::BlockNumber) -> ProviderResult<usize> {
            Ok(0)
        }

        fn get_by_hashes(&self, hashes: &[B256]) -> ProviderResult<Vec<Option<Bytes>>> {
            let entries = self.0.lock().unwrap();
            Ok(hashes.iter().map(|hash| entries.get(hash).cloned()).collect())
        }

        fn bal_stream(&self) -> BalNotificationStream {
            unreachable!("the test store does not publish notifications")
        }
    }

    fn config() -> BlockAccessListsConfig {
        BlockAccessListsConfig {
            downloader_enabled: true,
            downloader_request_limit: 2,
            downloader_max_concurrent_requests: 1,
            downloader_max_buffered_candidates: 4,
            downloader_min_transaction_count: 0,
            downloader_min_gas_used: 0,
        }
    }

    fn candidate(hash: B256, raw: &Bytes) -> HistoricalBalCandidate {
        HistoricalBalCandidate {
            block: NumHash::new(1, hash),
            expected_hash: keccak256(raw),
            transaction_count: 1,
            gas_used: 100_000,
        }
    }

    #[test]
    fn low_work_blocks_are_ineligible() {
        let raw = Bytes::from_static(&[0xc0]);
        let candidate = HistoricalBalCandidate {
            transaction_count: 50,
            gas_used: 50 * 21_000 - 1,
            ..candidate(B256::random(), &raw)
        };
        assert!(!candidate.is_eligible(BlockAccessListsConfig::default()));
        assert!(HistoricalBalCandidate { gas_used: 50 * 21_000, ..candidate }
            .is_eligible(BlockAccessListsConfig::default()));
    }

    #[tokio::test]
    async fn valid_missing_bal_is_persisted() {
        let client = TestFullBlockClient::default();
        let hash = B256::random();
        let raw = Bytes::from_static(&[0xc0]);
        client.insert_access_list(hash, raw.clone());
        let store = BalStoreHandle::new(TestBalStore::default());

        fetch_batch(
            client,
            store.clone(),
            config(),
            HistoricalBalFetchMetrics::default(),
            vec![candidate(hash, &raw)],
        )
        .await;

        assert_eq!(store.get_by_hash(hash).unwrap(), Some(raw));
    }

    #[tokio::test]
    async fn unavailable_and_invalid_bals_do_not_discard_a_valid_prefix() {
        let client = TestFullBlockClient::default();
        let valid_hash = B256::random();
        let unavailable_hash = B256::random();
        let invalid_hash = B256::random();
        let expected = Bytes::from_static(&[0xc0]);
        client.insert_access_list(valid_hash, expected.clone());
        client.insert_access_list(invalid_hash, Bytes::from_static(&[0xc1, 0x00]));
        let store = BalStoreHandle::new(TestBalStore::default());

        fetch_batch(
            client,
            store.clone(),
            config(),
            HistoricalBalFetchMetrics::default(),
            vec![
                candidate(valid_hash, &expected),
                candidate(unavailable_hash, &expected),
                candidate(invalid_hash, &expected),
            ],
        )
        .await;

        assert_eq!(store.get_by_hash(valid_hash).unwrap(), Some(expected));
        assert_eq!(store.get_by_hash(unavailable_hash).unwrap(), None);
        assert_eq!(store.get_by_hash(invalid_hash).unwrap(), None);
    }
}

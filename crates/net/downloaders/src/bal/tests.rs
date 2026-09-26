use super::*;
use alloy_consensus::Header;
use alloy_primitives::{Bytes, B256};
use reth_network_p2p::{
    block_access_lists::client::BalRequirement,
    download::DownloadClient,
    error::{PeerRequestResult, RequestError},
    priority::Priority,
    BlockAccessLists,
};
use reth_network_peers::{PeerId, WithPeerId};
use reth_provider::{test_utils::MockEthProvider, BalConfig, InMemoryBalStore};
use reth_storage_api::BalStore;
use std::{
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use tokio::sync::{mpsc, oneshot};

type Request = (Vec<B256>, oneshot::Sender<PeerRequestResult<BlockAccessLists>>);

#[derive(Debug, Clone)]
struct Client {
    requests: mpsc::UnboundedSender<Request>,
    bad_messages: Arc<AtomicUsize>,
}

impl DownloadClient for Client {
    fn report_bad_message(&self, _: PeerId) {
        self.bad_messages.fetch_add(1, Ordering::Relaxed);
    }
    fn num_connected_peers(&self) -> usize {
        1
    }
}

impl BlockAccessListsClient for Client {
    type Output = Pin<
        Box<dyn std::future::Future<Output = PeerRequestResult<BlockAccessLists>> + Send + Sync>,
    >;

    fn get_block_access_lists_with_priority_and_requirement(
        &self,
        hashes: Vec<B256>,
        _: Priority,
        requirement: BalRequirement,
    ) -> Self::Output {
        assert_eq!(requirement, BalRequirement::Optional);
        let (tx, rx) = oneshot::channel();
        self.requests.send((hashes, tx)).unwrap();
        Box::pin(async { rx.await.unwrap() })
    }
}

fn setup(count: u64) -> (BalDownloader<MockEthProvider, Client>, mpsc::UnboundedReceiver<Request>) {
    let provider = MockEthProvider::default();
    for number in 1..=count {
        let header =
            Header { number, block_access_list_hash: Some(raw().hash()), ..Default::default() };
        provider.add_header(header.hash_slow(), header);
    }
    let (requests, rx) = mpsc::unbounded_channel();
    let client = Client { requests, bad_messages: Arc::default() };
    (
        BalDownloader::new(
            provider,
            BalStoreHandle::new(InMemoryBalStore::default()),
            client,
            NonZeroUsize::new(2).unwrap(),
            100,
        ),
        rx,
    )
}

fn raw() -> RawBal {
    RawBal::new(Bytes::from_static(&[0xc0]))
}

fn set_header(provider: &MockEthProvider, header: Header) -> NumHash {
    provider.headers.lock().retain(|_, old| old.number != header.number);
    let block = NumHash::new(header.number, header.hash_slow());
    provider.add_header(block.hash, header);
    block
}

fn reply(tx: oneshot::Sender<PeerRequestResult<BlockAccessLists>>, values: Vec<Option<Bytes>>) {
    tx.send(Ok(WithPeerId::new(PeerId::default(), BlockAccessLists(values)))).unwrap();
}

async fn request(rx: &mut mpsc::UnboundedReceiver<Request>) -> Request {
    tokio::time::timeout(Duration::from_secs(2), rx.recv()).await.unwrap().unwrap()
}

#[tokio::test]
async fn fills_only_committed_store_misses_in_retention_window() {
    let (mut downloader, mut rx) = setup(6);
    downloader.retention = 3;
    let provider = downloader.provider.clone();
    let store = downloader.store.clone();
    let hit = provider.sealed_header(5).unwrap().unwrap();
    store.insert(hit.num_hash(), raw()).unwrap();
    set_header(&provider, Header { number: 3, ..Default::default() });

    let task = tokio::spawn(async move { downloader.backfill().await.unwrap() });
    let (hashes, tx) = request(&mut rx).await;
    let missing = provider.sealed_header(4).unwrap().unwrap();
    assert_eq!(hashes, vec![missing.hash(), provider.sealed_header(6).unwrap().unwrap().hash()]);
    reply(tx, vec![Some(raw().into_raw()); 2]);
    task.await.unwrap();
    assert_eq!(store.get_by_hash(missing.hash()).unwrap(), Some(raw().into_raw()));
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn short_responses_continue_and_unavailable_or_invalid_entries_retry() {
    let (mut downloader, mut rx) = setup(4);
    let store = downloader.store.clone();
    let bad = downloader.client.bad_messages.clone();
    let task = tokio::spawn(async move {
        downloader.backfill().await.unwrap();
        downloader
    });
    let (hashes, tx) = request(&mut rx).await;
    assert_eq!(hashes.len(), 4);
    reply(tx, vec![None, Some(raw().into_raw())]);
    let (suffix, tx) = request(&mut rx).await;
    assert_eq!(suffix, hashes[2..]);
    reply(tx, vec![Some(Bytes::from_static(&[0xff])), Some(raw().into_raw())]);
    let mut downloader = task.await.unwrap();
    assert_eq!(bad.load(Ordering::Relaxed), 1);
    assert!(store.get_by_hash(hashes[0]).unwrap().is_none());
    assert!(store.get_by_hash(hashes[2]).unwrap().is_none());
    let child = set_header(
        &downloader.provider,
        Header {
            number: 5,
            parent_hash: downloader.provider.block_hash(4).unwrap().unwrap(),
            block_access_list_hash: Some(raw().hash()),
            ..Default::default()
        },
    );
    store.insert(child, raw()).unwrap();
    let task = tokio::spawn(async move { downloader.backfill().await.unwrap() });
    let (retry, tx) = request(&mut rx).await;
    assert_eq!(retry, vec![hashes[0], hashes[2]]);
    reply(tx, vec![Some(raw().into_raw()); 2]);
    task.await.unwrap();
    assert!(store.get_by_hashes(&hashes).unwrap().iter().all(Option::is_some));
}

#[tokio::test]
async fn caps_concurrent_requests_and_reuses_finished_slot() {
    let (mut downloader, mut rx) = setup(40);
    let in_flight = Arc::new(metrics::atomics::AtomicU64::new(0));
    downloader.metrics.in_flight_requests = Gauge::from_arc(in_flight.clone());
    let task = tokio::spawn(async move { downloader.backfill().await.unwrap() });
    let (first, first_tx) = request(&mut rx).await;
    let (second, second_tx) = request(&mut rx).await;
    assert!(tokio::time::timeout(Duration::from_millis(30), rx.recv()).await.is_err());
    assert_eq!(f64::from_bits(in_flight.load(Ordering::Relaxed)), 2.);
    reply(second_tx, vec![Some(raw().into_raw()); second.len()]);
    let (third, third_tx) = request(&mut rx).await;
    assert!(tokio::time::timeout(Duration::from_millis(30), rx.recv()).await.is_err());
    assert_eq!(f64::from_bits(in_flight.load(Ordering::Relaxed)), 2.);
    reply(first_tx, vec![Some(raw().into_raw()); first.len()]);
    reply(third_tx, vec![Some(raw().into_raw()); third.len()]);
    task.await.unwrap();
    assert_eq!(f64::from_bits(in_flight.load(Ordering::Relaxed)), 0.);
    assert_eq!(first.len() + second.len() + third.len(), 40);
}

#[tokio::test]
async fn discards_reorged_and_expired_results() {
    let (mut downloader, mut rx) = setup(3);
    downloader.retention = 2;
    let provider = downloader.provider.clone();
    let store = downloader.store.clone();
    let task = tokio::spawn(async move { downloader.backfill().await.unwrap() });
    let (hashes, tx) = request(&mut rx).await;
    provider.headers.lock().remove(&hashes[1]);
    let replacement =
        Header { number: 2, extra_data: Bytes::from_static(b"reorg"), ..Default::default() };
    provider.add_header(replacement.hash_slow(), replacement);
    let new_tip = Header { number: 4, ..Default::default() };
    provider.add_header(new_tip.hash_slow(), new_tip);
    reply(tx, vec![Some(raw().into_raw()); 3]);
    task.await.unwrap();
    assert_eq!(store.get_by_hashes(&hashes).unwrap(), vec![None, None, Some(raw().into_raw())]);
}

#[tokio::test]
async fn head_rollback_discards_results_and_cancels_unreturned_suffix() {
    for response_len in [1, 3] {
        let (mut downloader, mut rx) = setup(3);
        let provider = downloader.provider.clone();
        let store = downloader.store.clone();
        let task = tokio::spawn(async move { downloader.backfill().await.unwrap() });
        let (hashes, tx) = request(&mut rx).await;
        provider.headers.lock().retain(|_, header| header.number <= 1);
        assert_eq!(provider.best_block_number().unwrap(), 1);
        reply(tx, vec![Some(raw().into_raw()); response_len]);
        tokio::time::timeout(Duration::from_secs(2), task).await.unwrap().unwrap();
        assert_eq!(store.get_by_hashes(&hashes).unwrap(), vec![Some(raw().into_raw()), None, None]);
        assert!(rx.try_recv().is_err());
    }
}

#[tokio::test]
async fn respects_store_pruning_ahead_of_canonical_head() {
    let (mut downloader, mut rx) = setup(3);
    let downloaded = Arc::new(metrics::atomics::AtomicU64::new(0));
    downloader.metrics.downloaded = Counter::from_arc(downloaded.clone());
    downloader.store =
        BalStoreHandle::new(InMemoryBalStore::new(BalConfig::with_in_memory_retention_distance(2)));
    let store = downloader.store.clone();
    let hashes = (1..=3)
        .map(|number| downloader.provider.sealed_header(number).unwrap().unwrap().hash())
        .collect::<Vec<_>>();
    // Validated payloads can enter the store before forkchoice advances the canonical head.
    store.insert(NumHash::new(4, B256::random()), raw()).unwrap();
    let task = tokio::spawn(async move {
        downloader.backfill().await.unwrap();
        downloader
    });
    let (requested, tx) = request(&mut rx).await;
    assert_eq!(requested, hashes[1..]);
    store.insert(NumHash::new(5, B256::random()), raw()).unwrap();
    reply(tx, vec![Some(raw().into_raw()); 2]);
    let mut downloader = task.await.unwrap();
    assert_eq!(store.get_by_hashes(&hashes).unwrap(), vec![None, None, Some(raw().into_raw())]);
    assert_eq!(downloaded.load(Ordering::Relaxed), 1);

    // The oldest missing BALs remain excluded on subsequent passes.
    tokio::time::timeout(Duration::from_secs(2), downloader.backfill()).await.unwrap().unwrap();
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn errors_empty_and_oversized_responses_do_not_stall_backfill() {
    for mode in 0..3 {
        let (mut downloader, mut rx) = setup(1);
        let store = downloader.store.clone();
        let task = tokio::spawn(async move { downloader.backfill().await.unwrap() });
        let (hashes, tx) = request(&mut rx).await;
        match mode {
            0 => tx.send(Err(RequestError::Timeout)).unwrap(),
            1 => reply(tx, vec![]),
            _ => reply(tx, vec![Some(raw().into_raw()); 2]),
        }
        task.await.unwrap();
        assert!(store.get_by_hash(hashes[0]).unwrap().is_none());
        assert!(rx.try_recv().is_err());
    }
}

#[tokio::test]
async fn startup_and_new_heads_fill_gaps_and_shutdown_stops_task() {
    let (downloader, mut rx) = setup(1);
    let provider = downloader.provider.clone();
    let store = downloader.store.clone();
    let (heads, head_rx) = mpsc::unbounded_channel();
    let task =
        tokio::spawn(downloader.run(tokio_stream::wrappers::UnboundedReceiverStream::new(head_rx)));
    let (_, tx) = request(&mut rx).await;
    reply(tx, vec![Some(raw().into_raw())]);
    let header = Header {
        number: 2,
        parent_hash: provider.block_hash(1).unwrap().unwrap(),
        block_access_list_hash: Some(raw().hash()),
        ..Default::default()
    };
    let hash = header.hash_slow();
    provider.add_header(hash, header);
    heads.send(()).unwrap();
    let (hashes, tx) = request(&mut rx).await;
    assert_eq!(hashes, vec![hash]);
    reply(tx, vec![Some(raw().into_raw())]);
    drop(heads);
    tokio::time::timeout(Duration::from_secs(2), task).await.unwrap().unwrap();
    assert!(store.get_by_hash(hash).unwrap().is_some());
}

#[derive(Debug, Default)]
struct TestStore {
    inner: InMemoryBalStore,
    flushes: AtomicUsize,
    reads: AtomicUsize,
    fail_flush: bool,
}

impl BalStore for TestStore {
    fn insert(&self, block: NumHash, bal: RawBal) -> ProviderResult<()> {
        self.inner.insert(block, bal)
    }

    fn flush(&self, blocks: &[NumHash]) -> ProviderResult<()> {
        let hashes = blocks.iter().map(|block| block.hash).collect::<Vec<_>>();
        if self.inner.get_by_hashes(&hashes)?.iter().any(Option::is_some) &&
            self.flushes.fetch_add(1, Ordering::Relaxed) == 0 &&
            self.fail_flush
        {
            return Err(reth_storage_api::errors::provider::ProviderError::InvalidStorageOutput)
        }
        Ok(())
    }

    fn prune(&self, tip: u64) -> ProviderResult<usize> {
        self.inner.prune(tip)
    }

    fn should_prune(&self, block: u64, tip: u64) -> bool {
        self.inner.should_prune(block, tip)
    }

    fn get_by_hashes(&self, hashes: &[B256]) -> ProviderResult<Vec<Option<Bytes>>> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.inner.get_by_hashes(hashes)
    }

    fn bal_stream(&self) -> reth_storage_api::BalNotificationStream {
        self.inner.bal_stream()
    }
}

#[tokio::test]
async fn retries_failed_flush_without_downloading_store_hits() {
    let (mut downloader, mut rx) = setup(1);
    let downloaded = Arc::new(metrics::atomics::AtomicU64::new(0));
    downloader.metrics.downloaded = Counter::from_arc(downloaded.clone());
    let store = Arc::new(TestStore { fail_flush: true, ..Default::default() });
    downloader.store = BalStoreHandle::new(store.clone());
    let task = tokio::spawn(async move {
        downloader.backfill().await.unwrap();
        downloader
    });
    let (_, tx) = request(&mut rx).await;
    reply(tx, vec![Some(raw().into_raw())]);
    let mut downloader = task.await.unwrap();
    assert_eq!(store.flushes.load(Ordering::Relaxed), 1);
    assert_eq!(downloaded.load(Ordering::Relaxed), 1);
    downloader.backfill().await.unwrap();
    assert_eq!(store.flushes.load(Ordering::Relaxed), 2);
    assert_eq!(downloaded.load(Ordering::Relaxed), 1);
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn skips_pre_bal_heads_and_only_checks_new_engine_insertions_when_caught_up() {
    let (mut downloader, mut rx) = setup(0);
    let store = Arc::new(TestStore::default());
    downloader.store = BalStoreHandle::new(store.clone());
    for number in 1..=32 {
        set_header(&downloader.provider, Header { number, ..Default::default() });
    }
    downloader.backfill().await.unwrap();
    assert_eq!(store.reads.load(Ordering::Relaxed), 0);

    for number in 33..=35 {
        let block = set_header(
            &downloader.provider,
            Header {
                number,
                parent_hash: downloader.provider.block_hash(number - 1).unwrap().unwrap(),
                block_access_list_hash: Some(raw().hash()),
                ..Default::default()
            },
        );
        store.insert(block, raw()).unwrap();
        downloader.backfill().await.unwrap();
        assert_eq!(store.reads.load(Ordering::Relaxed), (number - 32) as usize);
        assert_eq!(store.flushes.load(Ordering::Relaxed), 0);
        downloader.backfill().await.unwrap();
        assert_eq!(store.reads.load(Ordering::Relaxed), (number - 32) as usize);
    }
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn fills_skipped_blocks_even_when_new_head_is_already_stored() {
    let (mut downloader, mut rx) = setup(1);
    let store = downloader.store.clone();
    let parent = downloader.provider.sealed_header(1).unwrap().unwrap().num_hash();
    store.insert(parent, raw()).unwrap();
    downloader.backfill().await.unwrap();
    let missing = set_header(
        &downloader.provider,
        Header {
            number: 2,
            parent_hash: parent.hash,
            block_access_list_hash: Some(raw().hash()),
            ..Default::default()
        },
    );
    let tip = set_header(
        &downloader.provider,
        Header {
            number: 3,
            parent_hash: missing.hash,
            block_access_list_hash: Some(raw().hash()),
            ..Default::default()
        },
    );
    store.insert(tip, raw()).unwrap();
    let task = tokio::spawn(async move { downloader.backfill().await.unwrap() });
    let (hashes, tx) = request(&mut rx).await;
    assert_eq!(hashes, vec![missing.hash]);
    reply(tx, vec![Some(raw().into_raw())]);
    task.await.unwrap();
    assert!(store.get_by_hash(missing.hash).unwrap().is_some());
}

#[tokio::test]
async fn retries_above_activation_boundary_and_rescans_after_reorg() {
    let (mut downloader, mut rx) = setup(34);
    let store = Arc::new(TestStore::default());
    downloader.store = BalStoreHandle::new(store.clone());
    for number in 1..=32 {
        set_header(&downloader.provider, Header { number, ..Default::default() });
    }
    let task = tokio::spawn(async move {
        downloader.backfill().await.unwrap();
        downloader
    });
    let (hashes, tx) = request(&mut rx).await;
    assert_eq!(hashes.len(), 2);
    reply(tx, vec![None, Some(raw().into_raw())]);
    let mut downloader = task.await.unwrap();
    let reads = store.reads.load(Ordering::Relaxed);

    let task = tokio::spawn(async move {
        downloader.backfill().await.unwrap();
        downloader
    });
    let (retry, tx) = request(&mut rx).await;
    assert_eq!(retry, vec![hashes[0]]);
    assert_eq!(store.reads.load(Ordering::Relaxed), reads + 1);
    reply(tx, vec![Some(raw().into_raw())]);
    let mut downloader = task.await.unwrap();
    downloader.backfill().await.unwrap();
    assert_eq!(store.reads.load(Ordering::Relaxed), reads + 1);

    let replacements = (33..=34)
        .map(|number| {
            set_header(
                &downloader.provider,
                Header {
                    number,
                    block_access_list_hash: Some(raw().hash()),
                    extra_data: Bytes::from_static(b"reorg"),
                    ..Default::default()
                },
            )
            .hash
        })
        .collect::<Vec<_>>();
    // A one-block height increase on another branch is not an extension of the processed head.
    let new_tip = set_header(
        &downloader.provider,
        Header {
            number: 35,
            parent_hash: replacements[1],
            block_access_list_hash: Some(raw().hash()),
            ..Default::default()
        },
    );
    store.insert(new_tip, raw()).unwrap();
    let task = tokio::spawn(async move { downloader.backfill().await.unwrap() });
    let (hashes, tx) = request(&mut rx).await;
    assert_eq!(hashes, replacements);
    reply(tx, vec![Some(raw().into_raw()); 2]);
    task.await.unwrap();
    assert!(store.inner.get_by_hashes(&replacements).unwrap().iter().all(Option::is_some));
}

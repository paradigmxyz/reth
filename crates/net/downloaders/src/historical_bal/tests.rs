use super::*;
use alloy_consensus::Header;
use alloy_primitives::{keccak256, BlockHash, BlockNumber, Bytes};
use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
use reth_network_p2p::{error::RequestError, priority::Priority};
use reth_network_peers::{PeerId, WithPeerId};
use reth_provider::{test_utils::MockEthProvider, InMemoryBalStore};
use reth_stages_types::StageCheckpoint;
use reth_storage_api::{BalNotificationStream, BalStore};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt,
    future::Future,
    ops::RangeBounds,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
use tokio::sync::{oneshot, Notify};

impl<P, C, W> HistoricalBalWorker<P, C, W> {
    /// Sets the retry cooldown used after terminal unavailability, request failure, or invalidity.
    const fn with_retry_cooldown(mut self, retry_cooldown: Duration) -> Self {
        self.retry_cooldown = retry_cooldown;
        self
    }
}

#[derive(Debug, Clone, Default)]
struct TestProvider {
    state: Arc<Mutex<TestProviderState>>,
}

#[derive(Debug, Default)]
struct TestProviderState {
    bodies: u64,
    execution: u64,
    checkpoint_overrides: VecDeque<(u64, u64)>,
    canonical_results: VecDeque<Vec<bool>>,
    candidates: Vec<ScannedCandidate>,
    canonical: HashMap<u64, B256>,
    canonical_calls: usize,
    fail_canonical_call: Option<usize>,
    panic_canonical_call: Option<usize>,
    noncanonical_calls: HashSet<usize>,
}

impl TestProvider {
    fn with_candidates(bodies: u64, execution: u64, candidates: Vec<ScannedCandidate>) -> Self {
        let canonical = candidates
            .iter()
            .map(|candidate| (candidate.num_hash.number, candidate.num_hash.hash))
            .collect();
        Self {
            state: Arc::new(Mutex::new(TestProviderState {
                bodies,
                execution,
                checkpoint_overrides: VecDeque::new(),
                canonical_results: VecDeque::new(),
                candidates,
                canonical,
                canonical_calls: 0,
                fail_canonical_call: None,
                panic_canonical_call: None,
                noncanonical_calls: HashSet::new(),
            })),
        }
    }

    fn set_checkpoint_overrides(&self, checkpoints: impl IntoIterator<Item = (u64, u64)>) {
        self.state.lock().unwrap().checkpoint_overrides = checkpoints.into_iter().collect();
    }

    fn fail_canonical_call(&self, call: usize) {
        self.state.lock().unwrap().fail_canonical_call = Some(call);
    }

    fn panic_canonical_call(&self, call: usize) {
        self.state.lock().unwrap().panic_canonical_call = Some(call);
    }

    fn set_canonical_results(&self, results: impl IntoIterator<Item = Vec<bool>>) {
        self.state.lock().unwrap().canonical_results = results.into_iter().collect();
    }

    fn canonical_calls(&self) -> usize {
        self.state.lock().unwrap().canonical_calls
    }

    fn set_noncanonical_calls(&self, calls: impl IntoIterator<Item = usize>) {
        self.state.lock().unwrap().noncanonical_calls = calls.into_iter().collect();
    }

    fn set_canonical_hash(&self, number: BlockNumber, hash: B256) {
        self.state.lock().unwrap().canonical.insert(number, hash);
    }
}

/// `MockEthProvider`'s range body-index adapter is intentionally a no-op. Keep the blanket
/// provider test on a local wrapper so it exercises the production range-read path without
/// changing the storage test utility shared by other crates.
#[derive(Debug, Clone)]
struct RangeMockProvider {
    inner: MockEthProvider,
    canonical_ranges: Arc<Mutex<Vec<(BlockNumber, BlockNumber)>>>,
}

impl RangeMockProvider {
    fn new() -> Self {
        Self { inner: MockEthProvider::new(), canonical_ranges: Default::default() }
    }

    fn remove_body_indices(&self, number: BlockNumber) {
        self.inner.block_body_indices.lock().remove(&number);
    }

    fn remove_header(&self, hash: BlockHash) {
        self.inner.headers.lock().remove(&hash);
    }

    fn canonical_ranges(&self) -> Vec<(BlockNumber, BlockNumber)> {
        self.canonical_ranges.lock().unwrap().clone()
    }
}

impl HeaderProvider for RangeMockProvider {
    type Header = <MockEthProvider as HeaderProvider>::Header;

    fn header(
        &self,
        block_hash: BlockHash,
    ) -> reth_storage_api::errors::provider::ProviderResult<Option<Self::Header>> {
        self.inner.header(block_hash)
    }

    fn header_by_number(
        &self,
        number: BlockNumber,
    ) -> reth_storage_api::errors::provider::ProviderResult<Option<Self::Header>> {
        self.inner.header_by_number(number)
    }

    fn headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> reth_storage_api::errors::provider::ProviderResult<Vec<Self::Header>> {
        self.inner.headers_range(range)
    }

    fn sealed_header(
        &self,
        number: BlockNumber,
    ) -> reth_storage_api::errors::provider::ProviderResult<
        Option<reth_primitives_traits::SealedHeader<Self::Header>>,
    > {
        self.inner.sealed_header(number)
    }

    fn sealed_headers_while(
        &self,
        range: impl RangeBounds<BlockNumber>,
        predicate: impl FnMut(&reth_primitives_traits::SealedHeader<Self::Header>) -> bool,
    ) -> reth_storage_api::errors::provider::ProviderResult<
        Vec<reth_primitives_traits::SealedHeader<Self::Header>>,
    > {
        self.inner.sealed_headers_while(range, predicate)
    }
}

impl BlockHashReader for RangeMockProvider {
    fn block_hash(
        &self,
        number: BlockNumber,
    ) -> reth_storage_api::errors::provider::ProviderResult<Option<B256>> {
        self.inner.block_hash(number)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> reth_storage_api::errors::provider::ProviderResult<Vec<B256>> {
        self.canonical_ranges.lock().unwrap().push((start, end));
        self.inner.canonical_hashes_range(start, end)
    }
}

impl BlockBodyIndicesProvider for RangeMockProvider {
    fn block_body_indices(
        &self,
        number: BlockNumber,
    ) -> reth_storage_api::errors::provider::ProviderResult<
        Option<reth_db_api::models::StoredBlockBodyIndices>,
    > {
        self.inner.block_body_indices(number)
    }

    fn block_body_indices_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> reth_storage_api::errors::provider::ProviderResult<
        Vec<reth_db_api::models::StoredBlockBodyIndices>,
    > {
        let indices = self.inner.block_body_indices.lock();
        Ok(range.filter_map(|number| indices.get(&number).copied()).collect())
    }
}

impl StageCheckpointReader for RangeMockProvider {
    fn get_stage_checkpoint(
        &self,
        id: StageId,
    ) -> reth_storage_api::errors::provider::ProviderResult<Option<StageCheckpoint>> {
        self.inner.get_stage_checkpoint(id)
    }

    fn get_stage_checkpoint_progress(
        &self,
        id: StageId,
    ) -> reth_storage_api::errors::provider::ProviderResult<Option<Vec<u8>>> {
        self.inner.get_stage_checkpoint_progress(id)
    }

    fn get_all_checkpoints(
        &self,
    ) -> reth_storage_api::errors::provider::ProviderResult<Vec<(String, StageCheckpoint)>> {
        self.inner.get_all_checkpoints()
    }
}

impl HistoricalBalProvider for TestProvider {
    fn historical_bal_checkpoints(
        &self,
    ) -> reth_storage_api::errors::provider::ProviderResult<(u64, u64)> {
        let mut state = self.state.lock().unwrap();
        if let Some(checkpoints) = state.checkpoint_overrides.pop_front() {
            return Ok(checkpoints)
        }
        Ok((state.bodies, state.execution))
    }

    fn historical_bal_scan(
        &self,
        range: RangeInclusive<u64>,
        store: &BalStoreHandle,
        policy: BalExecutionPolicy,
    ) -> reth_storage_api::errors::provider::ProviderResult<HistoricalBalScanOutcome> {
        let candidates = self
            .state
            .lock()
            .unwrap()
            .candidates
            .iter()
            .copied()
            .filter(|candidate| range.contains(&candidate.num_hash.number))
            .collect::<Vec<_>>();
        let mut outcome = HistoricalBalScanOutcome::default();
        for candidate in candidates {
            if candidate.commitment.is_none() || !policy.is_eligible(candidate.transaction_count) {
                outcome.skipped += 1;
            } else {
                outcome.candidates.push(candidate);
            }
        }
        if outcome.candidates.is_empty() {
            return Ok(outcome)
        }
        let hashes =
            outcome.candidates.iter().map(|candidate| candidate.num_hash.hash).collect::<Vec<_>>();
        let stored = store.get_by_hashes(&hashes)?;
        assert_eq!(stored.len(), hashes.len());
        for (candidate, stored) in outcome.candidates.iter_mut().zip(stored) {
            candidate.store_miss = stored.is_none();
        }
        Ok(outcome)
    }

    fn historical_bal_canonical(
        &self,
        blocks: &[NumHash],
    ) -> reth_storage_api::errors::provider::ProviderResult<Vec<bool>> {
        let (call, should_panic) = {
            let mut state = self.state.lock().unwrap();
            state.canonical_calls += 1;
            let call = state.canonical_calls;
            let should_panic = state.panic_canonical_call == Some(call);
            (call, should_panic)
        };
        assert!(!should_panic, "scripted canonical provider panic at call {call}");

        let mut state = self.state.lock().unwrap();
        if let Some(result) = state.canonical_results.pop_front() {
            return Ok(result)
        }
        if state.fail_canonical_call == Some(state.canonical_calls) {
            return Err(reth_storage_api::errors::provider::ProviderError::InvalidStorageOutput)
        }
        if state.noncanonical_calls.contains(&state.canonical_calls) {
            return Ok(vec![false; blocks.len()])
        }
        Ok(blocks
            .iter()
            .map(|block| state.canonical.get(&block.number) == Some(&block.hash))
            .collect())
    }
}

#[derive(Clone, Default)]
struct ScriptedBalClient {
    state: Arc<Mutex<ScriptedBalClientState>>,
}

impl fmt::Debug for ScriptedBalClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScriptedBalClient").finish_non_exhaustive()
    }
}

#[derive(Default)]
struct ScriptedBalClientState {
    responses: VecDeque<ScriptedBalResponse>,
    requests: Vec<(Vec<B256>, BalRequirement)>,
    reports: Vec<PeerId>,
    active: usize,
    max_active: usize,
}

enum ScriptedBalResponse {
    Ready(PeerRequestResult<BlockAccessLists>),
    Pending(oneshot::Receiver<PeerRequestResult<BlockAccessLists>>),
}

impl ScriptedBalResponse {
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<PeerRequestResult<BlockAccessLists>> {
        match self {
            Self::Ready(response) => {
                Poll::Ready(std::mem::replace(response, Err(RequestError::ChannelClosed)))
            }
            Self::Pending(response) => Pin::new(response)
                .poll(cx)
                .map(|response| response.unwrap_or(Err(RequestError::ChannelClosed))),
        }
    }
}

impl ScriptedBalClient {
    fn push_response(&self, response: PeerRequestResult<BlockAccessLists>) {
        self.state.lock().unwrap().responses.push_back(ScriptedBalResponse::Ready(response));
    }

    fn push_pending_response(&self) -> oneshot::Sender<PeerRequestResult<BlockAccessLists>> {
        let (sender, receiver) = oneshot::channel();
        self.state.lock().unwrap().responses.push_back(ScriptedBalResponse::Pending(receiver));
        sender
    }

    fn requests(&self) -> Vec<(Vec<B256>, BalRequirement)> {
        self.state.lock().unwrap().requests.clone()
    }

    fn max_active(&self) -> usize {
        self.state.lock().unwrap().max_active
    }

    fn reports(&self) -> Vec<PeerId> {
        self.state.lock().unwrap().reports.clone()
    }
}

impl DownloadClient for ScriptedBalClient {
    fn report_bad_message(&self, peer_id: PeerId) {
        self.state.lock().unwrap().reports.push(peer_id);
    }

    fn num_connected_peers(&self) -> usize {
        1
    }
}

struct ScriptedBalFuture {
    response: Option<ScriptedBalResponse>,
    state: Arc<Mutex<ScriptedBalClientState>>,
}

impl Future for ScriptedBalFuture {
    type Output = PeerRequestResult<BlockAccessLists>;

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Poll::Ready(response) =
            self.response.as_mut().expect("future polled after completion").poll(_cx)
        else {
            return Poll::Pending
        };
        self.response = None;
        self.state.lock().unwrap().active -= 1;
        Poll::Ready(response)
    }
}

impl Drop for ScriptedBalFuture {
    fn drop(&mut self) {
        if self.response.is_some() {
            self.state.lock().unwrap().active -= 1;
        }
    }
}

impl BlockAccessListsClient for ScriptedBalClient {
    type Output = ScriptedBalFuture;

    fn get_block_access_lists_with_priority_and_requirement(
        &self,
        hashes: Vec<B256>,
        _priority: Priority,
        requirement: BalRequirement,
    ) -> Self::Output {
        let mut state = self.state.lock().unwrap();
        state.requests.push((hashes, requirement));
        state.active += 1;
        state.max_active = state.max_active.max(state.active);
        let response = state
            .responses
            .pop_front()
            .unwrap_or(ScriptedBalResponse::Ready(Err(RequestError::ChannelClosed)));
        ScriptedBalFuture { response: Some(response), state: Arc::clone(&self.state) }
    }
}

#[derive(Debug, Clone, Default)]
struct FaultingBalStore {
    inner: InMemoryBalStore,
    flushes: Arc<Mutex<Vec<Vec<NumHash>>>>,
    insert_notify: Option<Arc<Notify>>,
    lookup_calls: Arc<Mutex<usize>>,
    lookup_hashes: Arc<Mutex<Vec<Vec<BlockHash>>>>,
    truncate_lookup: bool,
    insert_on_second_lookup: Option<(NumHash, RawBal)>,
    partial_insert_error: bool,
    fail_flush: bool,
}

impl FaultingBalStore {
    fn flushes(&self) -> Vec<Vec<NumHash>> {
        self.flushes.lock().unwrap().clone()
    }

    fn lookups(&self) -> Vec<Vec<BlockHash>> {
        self.lookup_hashes.lock().unwrap().clone()
    }
}

impl BalStore for FaultingBalStore {
    fn insert(
        &self,
        num_hash: NumHash,
        bal: RawBal,
    ) -> reth_storage_api::errors::provider::ProviderResult<()> {
        self.inner.insert(num_hash, bal)
    }

    fn insert_many(
        &self,
        entries: Vec<(NumHash, RawBal)>,
    ) -> reth_storage_api::errors::provider::ProviderResult<()> {
        if self.partial_insert_error {
            if let Some((num_hash, bal)) = entries.into_iter().next() {
                self.inner.insert(num_hash, bal)?;
            }
            return Err(reth_storage_api::errors::provider::ProviderError::InvalidStorageOutput)
        }
        self.inner.insert_many(entries)?;
        if let Some(notify) = &self.insert_notify {
            notify.notify_one();
        }
        Ok(())
    }

    fn flush(&self, blocks: &[NumHash]) -> reth_storage_api::errors::provider::ProviderResult<()> {
        self.flushes.lock().unwrap().push(blocks.to_vec());
        if self.fail_flush {
            Err(reth_storage_api::errors::provider::ProviderError::InvalidStorageOutput)
        } else {
            Ok(())
        }
    }

    fn prune(&self, tip: BlockNumber) -> reth_storage_api::errors::provider::ProviderResult<usize> {
        self.inner.prune(tip)
    }

    fn get_by_hashes(
        &self,
        block_hashes: &[BlockHash],
    ) -> reth_storage_api::errors::provider::ProviderResult<Vec<Option<Bytes>>> {
        self.lookup_hashes.lock().unwrap().push(block_hashes.to_vec());
        let lookup = {
            let mut lookup_calls = self.lookup_calls.lock().unwrap();
            *lookup_calls += 1;
            *lookup_calls
        };
        if lookup == 2 &&
            let Some((num_hash, bal)) = &self.insert_on_second_lookup
        {
            self.inner.insert(*num_hash, bal.clone())?;
        }
        let mut values = self.inner.get_by_hashes(block_hashes)?;
        if self.truncate_lookup {
            values.pop();
        }
        Ok(values)
    }

    fn bal_stream(&self) -> BalNotificationStream {
        self.inner.bal_stream()
    }
}

/// Models stores that retain identities marked canonical when a flush fails and replay all of
/// those identities on a later flush, as the production `RocksDB` store does.
#[derive(Debug, Clone, Default)]
struct RetainingFlushBalStore {
    inner: InMemoryBalStore,
    pending: Arc<Mutex<HashMap<NumHash, RawBal>>>,
    canonical_pending: Arc<Mutex<HashSet<NumHash>>>,
    durable: Arc<Mutex<HashMap<NumHash, RawBal>>>,
    flushes: Arc<Mutex<Vec<Vec<NumHash>>>>,
    fail_next_flush: Arc<Mutex<bool>>,
}

impl RetainingFlushBalStore {
    fn with_failed_first_flush() -> Self {
        Self { fail_next_flush: Arc::new(Mutex::new(true)), ..Default::default() }
    }

    fn durable(&self, block: NumHash) -> Option<RawBal> {
        self.durable.lock().unwrap().get(&block).cloned()
    }

    fn flushes(&self) -> Vec<Vec<NumHash>> {
        self.flushes.lock().unwrap().clone()
    }
}

impl BalStore for RetainingFlushBalStore {
    fn insert(
        &self,
        num_hash: NumHash,
        bal: RawBal,
    ) -> reth_storage_api::errors::provider::ProviderResult<()> {
        self.pending.lock().unwrap().insert(num_hash, bal.clone());
        self.canonical_pending.lock().unwrap().remove(&num_hash);
        self.inner.insert(num_hash, bal)
    }

    fn insert_many(
        &self,
        entries: Vec<(NumHash, RawBal)>,
    ) -> reth_storage_api::errors::provider::ProviderResult<()> {
        {
            let mut pending = self.pending.lock().unwrap();
            let mut canonical_pending = self.canonical_pending.lock().unwrap();
            for (num_hash, bal) in &entries {
                pending.insert(*num_hash, bal.clone());
                canonical_pending.remove(num_hash);
            }
        }
        self.inner.insert_many(entries)
    }

    fn flush(&self, blocks: &[NumHash]) -> reth_storage_api::errors::provider::ProviderResult<()> {
        self.flushes.lock().unwrap().push(blocks.to_vec());
        {
            let pending = self.pending.lock().unwrap();
            self.canonical_pending
                .lock()
                .unwrap()
                .extend(blocks.iter().copied().filter(|block| pending.contains_key(block)));
        }

        let mut fail_next = self.fail_next_flush.lock().unwrap();
        if *fail_next {
            *fail_next = false;
            return Err(reth_storage_api::errors::provider::ProviderError::InvalidStorageOutput)
        }
        drop(fail_next);

        let pending = self.pending.lock().unwrap();
        let canonical_pending = self.canonical_pending.lock().unwrap();
        let mut durable = self.durable.lock().unwrap();
        for block in canonical_pending.iter().copied() {
            if let Some(bal) = pending.get(&block) {
                durable.insert(block, bal.clone());
            }
        }
        drop(durable);
        drop(canonical_pending);
        drop(pending);
        self.canonical_pending.lock().unwrap().clear();
        Ok(())
    }

    fn prune(&self, tip: BlockNumber) -> reth_storage_api::errors::provider::ProviderResult<usize> {
        self.inner.prune(tip)
    }

    fn get_by_hashes(
        &self,
        block_hashes: &[BlockHash],
    ) -> reth_storage_api::errors::provider::ProviderResult<Vec<Option<Bytes>>> {
        self.inner.get_by_hashes(block_hashes)
    }

    fn bal_stream(&self) -> BalNotificationStream {
        self.inner.bal_stream()
    }
}

fn candidate(
    number: u64,
    commitment: Option<B256>,
    tx_count: u64,
    store_miss: bool,
) -> ScannedCandidate {
    ScannedCandidate {
        num_hash: NumHash::new(number, B256::with_last_byte(number as u8)),
        commitment,
        transaction_count: tx_count,
        store_miss,
    }
}

fn eligible_candidate(number: u64, commitment: B256) -> EligibleCandidate {
    EligibleCandidate {
        num_hash: NumHash::new(number, B256::with_last_byte(number as u8)),
        commitment,
    }
}

fn worker_config(enabled: bool) -> HistoricalBalWorkerConfig {
    HistoricalBalWorkerConfig::new(enabled, 1, 2, 2, 8).unwrap()
}

fn test_worker<P, C>(
    provider: P,
    store: BalStoreHandle,
    client: C,
    config: HistoricalBalWorkerConfig,
) -> HistoricalBalWorker<P, C, futures::stream::Empty<()>> {
    HistoricalBalWorker::from_parts(
        provider,
        store,
        client,
        futures::stream::empty(),
        Runtime::test(),
        config,
    )
}

fn successful_response(values: Vec<Option<Bytes>>) -> PeerRequestResult<BlockAccessLists> {
    Ok(WithPeerId::new(PeerId::random(), BlockAccessLists(values)))
}

fn counter_values(snapshotter: &Snapshotter) -> HashMap<String, u64> {
    snapshotter
        .snapshot()
        .into_vec()
        .into_iter()
        .filter(|(key, _, _, _)| key.key().name().starts_with("downloaders.historical_bal."))
        .map(|(key, _, _, value)| {
            let DebugValue::Counter(value) = value else {
                panic!("historical BAL metric was not a counter")
            };
            (key.key().name().to_string(), value)
        })
        .collect()
}

fn add_mock_header(
    provider: &RangeMockProvider,
    number: u64,
    commitment: Option<B256>,
    transaction_count: u64,
) -> B256 {
    let header = Header { number, block_access_list_hash: commitment, ..Default::default() };
    let hash = header.hash_slow();
    provider.inner.add_header(hash, header);
    let mut indices = provider.inner.block_body_indices(number).unwrap().unwrap_or_default();
    indices.tx_count = transaction_count;
    provider.inner.add_block_body_indices(number, indices);
    hash
}

#[tokio::test]
async fn disabled_worker_sends_no_requests() {
    let raw = Bytes::from_static(&[0xc0]);
    let provider =
        TestProvider::with_candidates(1, 0, vec![candidate(1, Some(keccak256(&raw)), 1, true)]);
    let store = BalStoreHandle::new(InMemoryBalStore::default());
    let client = ScriptedBalClient::default();
    let mut worker = HistoricalBalWorker::from_parts(
        provider,
        store,
        client.clone(),
        futures::stream::iter([(), ()]),
        Runtime::test(),
        worker_config(false),
    );

    worker.run_once().await;
    worker.run().await;

    assert!(client.requests().is_empty());
}

#[tokio::test]
async fn enabled_startup_downloads_store_miss_and_restart_skips_store_hit() {
    let raw = Bytes::from_static(&[0xc0]);
    let expected = keccak256(&raw);
    let candidate = candidate(1, Some(expected), 1, true);
    let provider = TestProvider::with_candidates(1, 0, vec![candidate]);
    let store = BalStoreHandle::new(InMemoryBalStore::default());
    let client = ScriptedBalClient::default();
    client.push_response(successful_response(vec![Some(raw.clone())]));

    test_worker(provider.clone(), store.clone(), client.clone(), worker_config(true)).run().await;

    let requests = client.requests();
    assert_eq!(requests, vec![(vec![candidate.num_hash.hash], BalRequirement::Optional)]);
    assert_eq!(store.get_by_hash(candidate.num_hash.hash).unwrap(), Some(raw));

    test_worker(provider, store, client.clone(), worker_config(true)).run().await;

    assert_eq!(client.requests().len(), 1);
}

#[tokio::test]
async fn later_wakeup_reconciles_missed_committed_progress() {
    let raw = Bytes::from_static(&[0xc0]);
    let candidate = candidate(1, Some(keccak256(&raw)), 1, true);
    let provider = TestProvider::with_candidates(1, 0, vec![candidate]);
    provider.set_checkpoint_overrides([(0, 0), (1, 0), (1, 0)]);
    let store = BalStoreHandle::new(InMemoryBalStore::default());
    let client = ScriptedBalClient::default();
    client.push_response(successful_response(vec![Some(raw.clone())]));
    let worker = HistoricalBalWorker::from_parts(
        provider,
        store.clone(),
        client.clone(),
        futures::stream::iter([()]),
        Runtime::test(),
        worker_config(true),
    );

    worker.run().await;

    assert_eq!(client.requests(), vec![(vec![candidate.num_hash.hash], BalRequirement::Optional)]);
    assert_eq!(store.get_by_hash(candidate.num_hash.hash).unwrap(), Some(raw));
}

#[tokio::test]
async fn request_error_does_not_stop_later_reconciliation() {
    let raw = Bytes::from_static(&[0xc0]);
    let candidate = candidate(1, Some(keccak256(&raw)), 1, true);
    let provider = TestProvider::with_candidates(1, 0, vec![candidate]);
    let store = BalStoreHandle::new(InMemoryBalStore::default());
    let client = ScriptedBalClient::default();
    client.push_response(Err(RequestError::Timeout));
    client.push_response(successful_response(vec![Some(raw.clone())]));
    let mut worker = test_worker(provider, store.clone(), client.clone(), worker_config(true))
        .with_retry_cooldown(Duration::ZERO);

    worker.run_once().await;
    worker.run_once().await;

    assert_eq!(client.requests().len(), 2);
    assert_eq!(store.get_by_hash(candidate.num_hash.hash).unwrap(), Some(raw));
}

#[tokio::test]
async fn metrics_count_requested_downloaded_skipped_and_unavailable() {
    let raw = Bytes::from_static(&[0xc0]);
    let expected = keccak256(&raw);
    let valid = candidate(1, Some(expected), 1, true);
    let unavailable = candidate(2, Some(expected), 1, true);
    let low_work = candidate(3, Some(expected), 0, true);
    let provider = TestProvider::with_candidates(3, 0, vec![valid, unavailable, low_work]);
    let client = ScriptedBalClient::default();
    client.push_response(successful_response(vec![Some(raw), None]));
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let mut worker = test_worker(
        provider,
        BalStoreHandle::new(InMemoryBalStore::default()),
        client,
        worker_config(true),
    );
    worker.metrics = metrics::with_local_recorder(&recorder, || {
        HistoricalBalDownloaderMetrics::new_with_labels(Vec::<metrics::Label>::new())
    });

    worker.run_once().await;

    assert_eq!(
        counter_values(&snapshotter),
        HashMap::from([
            ("downloaders.historical_bal.requested".to_string(), 2),
            ("downloaders.historical_bal.downloaded".to_string(), 1),
            ("downloaders.historical_bal.skipped".to_string(), 1),
            ("downloaders.historical_bal.unavailable".to_string(), 1),
            ("downloaders.historical_bal.invalid".to_string(), 0),
        ])
    );
}

#[tokio::test]
async fn metrics_count_invalid_response_and_report_peer_once() {
    let candidate = candidate(1, Some(B256::ZERO), 1, true);
    let provider = TestProvider::with_candidates(1, 0, vec![candidate]);
    let client = ScriptedBalClient::default();
    let peer = PeerId::random();
    client.push_response(Ok(WithPeerId::new(peer, BlockAccessLists(vec![None, None]))));
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let mut worker = test_worker(
        provider,
        BalStoreHandle::new(InMemoryBalStore::default()),
        client.clone(),
        worker_config(true),
    );
    worker.metrics = metrics::with_local_recorder(&recorder, || {
        HistoricalBalDownloaderMetrics::new_with_labels(Vec::<metrics::Label>::new())
    });

    worker.run_once().await;

    assert_eq!(
        counter_values(&snapshotter).get("downloaders.historical_bal.invalid").copied(),
        Some(1)
    );
    assert_eq!(client.reports(), vec![peer]);
}

#[tokio::test]
async fn metrics_count_all_candidates_abandoned_after_recheck_error() {
    let raw = Bytes::from_static(&[0xc0]);
    let expected = keccak256(&raw);
    let candidates =
        (1..=3).map(|number| candidate(number, Some(expected), 1, true)).collect::<Vec<_>>();
    let provider = TestProvider::with_candidates(3, 0, candidates);
    provider.fail_canonical_call(2);
    let client = ScriptedBalClient::default();
    client.push_response(successful_response(vec![Some(raw)]));
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let config = HistoricalBalWorkerConfig::new(true, 1, 1, 2, 3).unwrap();
    let mut worker =
        test_worker(provider, BalStoreHandle::new(InMemoryBalStore::default()), client, config);
    worker.metrics = metrics::with_local_recorder(&recorder, || {
        HistoricalBalDownloaderMetrics::new_with_labels(Vec::<metrics::Label>::new())
    });

    worker.run_once().await;

    let counters = counter_values(&snapshotter);
    assert_eq!(counters.get("downloaders.historical_bal.requested"), Some(&1));
    assert_eq!(counters.get("downloaders.historical_bal.downloaded"), Some(&1));
    assert_eq!(counters.get("downloaders.historical_bal.skipped"), Some(&2));
}

#[tokio::test]
async fn execution_progress_drops_later_batch_before_dispatch() {
    let raw = Bytes::from_static(&[0xc0]);
    let expected = keccak256(&raw);
    let first = candidate(1, Some(expected), 1, true);
    let second = candidate(2, Some(expected), 1, true);
    let provider = TestProvider::with_candidates(2, 0, vec![first, second]);
    provider.set_checkpoint_overrides([(2, 0), (2, 0), (2, 2)]);
    let store = BalStoreHandle::new(InMemoryBalStore::default());
    let client = ScriptedBalClient::default();
    for _ in 0..2 {
        client.push_response(successful_response(vec![Some(raw.clone())]));
    }
    let config = HistoricalBalWorkerConfig::new(true, 1, 1, 1, 2).unwrap();
    let mut worker = test_worker(provider, store, client.clone(), config);

    worker.run_once().await;

    assert_eq!(client.requests(), vec![(vec![first.num_hash.hash], BalRequirement::Optional)]);
}

#[tokio::test]
async fn later_batch_recheck_error_preserves_in_flight_response() {
    let raw = Bytes::from_static(&[0xc0]);
    let expected = keccak256(&raw);
    let first = candidate(1, Some(expected), 1, true);
    let second = candidate(2, Some(expected), 1, true);
    let provider = TestProvider::with_candidates(2, 0, vec![first, second]);
    provider.fail_canonical_call(2);
    let store = BalStoreHandle::new(InMemoryBalStore::default());
    let client = ScriptedBalClient::default();
    client.push_response(successful_response(vec![Some(raw.clone())]));
    let config = HistoricalBalWorkerConfig::new(true, 1, 1, 2, 2).unwrap();
    let mut worker = test_worker(provider, store.clone(), client.clone(), config);

    worker.run_once().await;

    assert_eq!(client.requests(), vec![(vec![first.num_hash.hash], BalRequirement::Optional)]);
    assert_eq!(store.get_by_hash(first.num_hash.hash).unwrap(), Some(raw));
}

#[tokio::test]
async fn predispatch_recheck_drops_reorged_candidate() {
    let raw = Bytes::from_static(&[0xc0]);
    let candidate = candidate(1, Some(keccak256(&raw)), 1, true);
    let provider = TestProvider::with_candidates(1, 0, vec![candidate]);
    provider.set_noncanonical_calls([1]);
    let client = ScriptedBalClient::default();
    let mut worker = test_worker(
        provider,
        BalStoreHandle::new(InMemoryBalStore::default()),
        client.clone(),
        worker_config(true),
    );

    worker.run_once().await;

    assert!(client.requests().is_empty());
}

#[tokio::test]
async fn predispatch_recheck_drops_new_store_hit() {
    let raw = Bytes::from_static(&[0xc0]);
    let candidate = candidate(1, Some(keccak256(&raw)), 1, true);
    let provider = TestProvider::with_candidates(1, 0, vec![candidate]);
    let changing_store = FaultingBalStore {
        insert_on_second_lookup: Some((candidate.num_hash, RawBal::new(raw.clone()))),
        ..Default::default()
    };
    let store = BalStoreHandle::new(changing_store);
    let client = ScriptedBalClient::default();
    let mut worker = test_worker(provider, store.clone(), client.clone(), worker_config(true));

    worker.run_once().await;

    assert!(client.requests().is_empty());
    assert_eq!(store.get_by_hash(candidate.num_hash.hash).unwrap(), Some(raw));
}

#[tokio::test]
async fn partial_insert_error_still_rechecks_and_flushes() {
    let raw = Bytes::from_static(&[0xc0]);
    let expected = keccak256(&raw);
    let candidate = candidate(1, Some(expected), 1, true);
    let provider = TestProvider::with_candidates(1, 0, vec![candidate]);
    let fault_store = FaultingBalStore { partial_insert_error: true, ..Default::default() };
    let store = BalStoreHandle::new(fault_store.clone());
    let client = ScriptedBalClient::default();
    client.push_response(successful_response(vec![Some(raw.clone())]));
    let mut worker = test_worker(provider.clone(), store.clone(), client, worker_config(true));

    worker.run_once().await;

    assert_eq!(store.get_by_hash(candidate.num_hash.hash).unwrap(), Some(raw));
    assert_eq!(provider.canonical_calls(), 3);
    assert_eq!(fault_store.flushes(), vec![vec![candidate.num_hash]]);
}

#[tokio::test]
async fn post_insert_canonical_error_retries_flush_on_next_wakeup() {
    let raw = Bytes::from_static(&[0xc0]);
    let candidate = candidate(1, Some(keccak256(&raw)), 1, true);
    let provider = TestProvider::with_candidates(1, 0, vec![candidate]);
    provider.fail_canonical_call(3);
    let fault_store = FaultingBalStore::default();
    let store = BalStoreHandle::new(fault_store.clone());
    let client = ScriptedBalClient::default();
    client.push_response(successful_response(vec![Some(raw.clone())]));
    let worker = HistoricalBalWorker::from_parts(
        provider.clone(),
        store.clone(),
        client.clone(),
        futures::stream::iter([()]),
        Runtime::test(),
        worker_config(true),
    );

    worker.run().await;

    assert_eq!(client.requests(), vec![(vec![candidate.num_hash.hash], BalRequirement::Optional)]);
    assert_eq!(store.get_by_hash(candidate.num_hash.hash).unwrap(), Some(raw));
    assert_eq!(provider.canonical_calls(), 4);
    assert_eq!(fault_store.flushes(), vec![vec![candidate.num_hash]]);
}

#[tokio::test]
async fn post_insert_canonical_error_drops_stale_identity_without_flush() {
    let raw = Bytes::from_static(&[0xc0]);
    let candidate = candidate(1, Some(keccak256(&raw)), 1, true);
    let provider = TestProvider::with_candidates(1, 0, vec![candidate]);
    provider.fail_canonical_call(3);
    provider.set_noncanonical_calls([4]);
    let fault_store = FaultingBalStore::default();
    let store = BalStoreHandle::new(fault_store.clone());
    let client = ScriptedBalClient::default();
    client.push_response(successful_response(vec![Some(raw)]));
    let mut worker = test_worker(provider.clone(), store, client.clone(), worker_config(true));

    worker.run_once().await;
    worker.run_once().await;

    assert_eq!(client.requests().len(), 1);
    assert_eq!(provider.canonical_calls(), 4);
    assert!(fault_store.flushes().is_empty());
    assert!(worker.pending_flush.is_empty());
}

#[tokio::test]
async fn post_insert_canonical_panic_drops_stale_identity_without_flush() {
    let raw = Bytes::from_static(&[0xc0]);
    let candidate = candidate(1, Some(keccak256(&raw)), 1, true);
    let provider = TestProvider::with_candidates(1, 0, vec![candidate]);
    provider.panic_canonical_call(3);
    provider.set_noncanonical_calls([4]);
    let fault_store = FaultingBalStore::default();
    let store = BalStoreHandle::new(fault_store.clone());
    let client = ScriptedBalClient::default();
    client.push_response(successful_response(vec![Some(raw)]));
    let mut worker = test_worker(provider.clone(), store, client.clone(), worker_config(true));

    worker.run_once().await;
    worker.run_once().await;

    assert_eq!(client.requests().len(), 1);
    assert_eq!(provider.canonical_calls(), 4);
    assert!(fault_store.flushes().is_empty());
    assert!(worker.pending_flush.is_empty());
    assert!(worker.uncertain_flush.is_empty());
}

#[tokio::test]
async fn failed_flush_does_not_replay_reorged_identity_on_retry() {
    let raw = Bytes::from_static(&[0xc0]);
    let candidate = candidate(1, Some(keccak256(&raw)), 1, true);
    let provider = TestProvider::with_candidates(1, 0, vec![candidate]);
    provider.set_noncanonical_calls([4]);
    let retaining_store = RetainingFlushBalStore::with_failed_first_flush();
    let store = BalStoreHandle::new(retaining_store.clone());
    let client = ScriptedBalClient::default();
    client.push_response(successful_response(vec![Some(raw.clone())]));
    let worker = HistoricalBalWorker::from_parts(
        provider,
        store,
        client.clone(),
        futures::stream::iter([()]),
        Runtime::test(),
        worker_config(true),
    );

    worker.run().await;

    assert_eq!(client.requests().len(), 1);
    assert_eq!(retaining_store.durable(candidate.num_hash), None);
    assert_eq!(retaining_store.flushes(), vec![vec![candidate.num_hash]]);
}

#[tokio::test]
async fn uncertain_flush_waits_for_all_identities_to_be_canonical() {
    let raw = Bytes::from_static(&[0xc0]);
    let expected = keccak256(&raw);
    let first = candidate(1, Some(expected), 1, true);
    let second = candidate(2, Some(expected), 1, true);
    let provider = TestProvider::with_candidates(2, 0, vec![first, second]);
    let retaining_store = RetainingFlushBalStore::with_failed_first_flush();
    let store = BalStoreHandle::new(retaining_store.clone());
    let client = ScriptedBalClient::default();
    client.push_response(successful_response(vec![Some(raw.clone()), Some(raw.clone())]));
    let config = HistoricalBalWorkerConfig::new(true, 1, 2, 1, 2).unwrap();
    let mut worker = test_worker(provider.clone(), store, client.clone(), config);

    worker.run_once().await;
    provider.set_canonical_hash(second.num_hash.number, B256::ZERO);
    worker.run_once().await;

    assert_eq!(client.requests().len(), 1);
    assert_eq!(retaining_store.flushes().len(), 1);
    assert_eq!(retaining_store.durable(first.num_hash), None);
    assert_eq!(retaining_store.durable(second.num_hash), None);
    assert_eq!(worker.pending_flush.len(), 2);

    provider.set_canonical_hash(second.num_hash.number, second.num_hash.hash);
    worker.run_once().await;

    assert_eq!(retaining_store.flushes().len(), 2);
    assert_eq!(retaining_store.durable(first.num_hash), Some(RawBal::new(raw.clone())));
    assert_eq!(retaining_store.durable(second.num_hash), Some(RawBal::new(raw)));
    assert!(worker.pending_flush.is_empty());
    assert!(worker.uncertain_flush.is_empty());
}

#[tokio::test]
async fn uncertain_flush_does_not_block_canonical_new_work() {
    let raw = Bytes::from_static(&[0xc0]);
    let expected = keccak256(&raw);
    let first = candidate(1, Some(expected), 1, true);
    let second = candidate(2, Some(expected), 1, true);
    let provider = TestProvider::with_candidates(2, 0, vec![first, second]);
    provider.set_canonical_results([
        vec![true, true],
        vec![true],
        vec![true],
        vec![true],
        vec![true],
        vec![true, false],
    ]);
    let retaining_store = RetainingFlushBalStore::with_failed_first_flush();
    let store = BalStoreHandle::new(retaining_store.clone());
    let client = ScriptedBalClient::default();
    client.push_response(successful_response(vec![Some(raw.clone())]));
    client.push_response(successful_response(vec![Some(raw.clone())]));
    let config = HistoricalBalWorkerConfig::new(true, 1, 2, 1, 2).unwrap();
    let mut worker = test_worker(provider, store, client.clone(), config);

    worker.run_once().await;

    assert_eq!(client.requests().len(), 2);
    assert!(worker.pending_flush.is_empty());
    assert!(worker.uncertain_flush.is_empty());
    assert_eq!(retaining_store.durable(first.num_hash), Some(RawBal::new(raw)));
    assert_eq!(retaining_store.durable(second.num_hash), None);
    assert_eq!(retaining_store.flushes(), vec![vec![first.num_hash], vec![first.num_hash]]);
}

#[tokio::test]
async fn pending_flush_capacity_bounds_new_work() {
    let raw = Bytes::from_static(&[0xc0]);
    let expected = keccak256(&raw);
    let candidates =
        (1..=4).map(|number| candidate(number, Some(expected), 1, true)).collect::<Vec<_>>();
    let provider = TestProvider::with_candidates(4, 0, candidates[..2].to_vec());
    provider.set_checkpoint_overrides([(2, 0), (2, 0), (4, 2)]);
    provider.set_noncanonical_calls(4..=8);
    let retaining_store = RetainingFlushBalStore::with_failed_first_flush();
    let store = BalStoreHandle::new(retaining_store);
    let client = ScriptedBalClient::default();
    client.push_response(successful_response(vec![Some(raw.clone()), Some(raw.clone())]));
    let config = HistoricalBalWorkerConfig::new(true, 1, 2, 1, 2).unwrap();
    let mut worker = test_worker(provider.clone(), store, client.clone(), config);

    worker.run_once().await;
    assert_eq!(worker.pending_flush.len(), 2);
    assert_eq!(client.requests().len(), 1);

    provider.state.lock().unwrap().candidates.extend_from_slice(&candidates[2..]);
    worker.run_once().await;
    worker.run_once().await;

    assert_eq!(worker.pending_flush.len(), 2);
    assert_eq!(client.requests().len(), 1);
}

#[tokio::test]
async fn flush_error_does_not_prevent_next_reconciliation() {
    let raw = Bytes::from_static(&[0xc0]);
    let expected = keccak256(&raw);
    let first = candidate(1, Some(expected), 1, true);
    let second = candidate(2, Some(expected), 1, true);
    let provider = TestProvider::with_candidates(2, 0, vec![first, second]);
    let fault_store =
        FaultingBalStore { partial_insert_error: true, fail_flush: true, ..Default::default() };
    let store = BalStoreHandle::new(fault_store.clone());
    let client = ScriptedBalClient::default();
    client.push_response(successful_response(vec![Some(raw.clone()), Some(raw.clone())]));
    client.push_response(successful_response(vec![Some(raw.clone())]));
    let mut worker = test_worker(provider, store.clone(), client.clone(), worker_config(true))
        .with_retry_cooldown(Duration::ZERO);

    worker.run_once().await;
    worker.run_once().await;

    assert_eq!(
        client.requests(),
        vec![
            (vec![first.num_hash.hash, second.num_hash.hash], BalRequirement::Optional),
            (vec![second.num_hash.hash], BalRequirement::Optional),
        ]
    );
    assert_eq!(store.get_by_hash(first.num_hash.hash).unwrap(), Some(raw.clone()));
    assert_eq!(store.get_by_hash(second.num_hash.hash).unwrap(), Some(raw));
    assert_eq!(fault_store.flushes().len(), 3);
}

#[tokio::test]
async fn post_response_reorg_rejects_stale_bal() {
    let raw = Bytes::from_static(&[0xc0]);
    let candidate = candidate(1, Some(keccak256(&raw)), 1, true);
    let provider = TestProvider::with_candidates(1, 0, vec![candidate]);
    provider.set_noncanonical_calls([2]);
    let store = BalStoreHandle::new(InMemoryBalStore::default());
    let client = ScriptedBalClient::default();
    client.push_response(successful_response(vec![Some(raw)]));
    let mut worker = test_worker(provider.clone(), store.clone(), client, worker_config(true));

    worker.run_once().await;

    assert_eq!(provider.canonical_calls(), 2);
    assert_eq!(store.get_by_hash(candidate.num_hash.hash).unwrap(), None);
}

#[tokio::test]
async fn pre_flush_reorg_excludes_stale_block() {
    let raw = Bytes::from_static(&[0xc0]);
    let candidate = candidate(1, Some(keccak256(&raw)), 1, true);
    let provider = TestProvider::with_candidates(1, 0, vec![candidate]);
    provider.set_noncanonical_calls([3]);
    let recording_store = FaultingBalStore::default();
    let store = BalStoreHandle::new(recording_store.clone());
    let client = ScriptedBalClient::default();
    client.push_response(successful_response(vec![Some(raw.clone())]));
    let mut worker = test_worker(provider.clone(), store.clone(), client, worker_config(true));

    worker.run_once().await;

    assert_eq!(provider.canonical_calls(), 3);
    assert_eq!(store.get_by_hash(candidate.num_hash.hash).unwrap(), Some(raw));
    assert!(recording_store.flushes().is_empty());
}

#[tokio::test]
async fn request_batching_and_future_construction_respect_bounds() {
    let raw = Bytes::from_static(&[0xc0]);
    let expected = keccak256(&raw);
    let candidates =
        (1..=5).map(|number| candidate(number, Some(expected), 1, true)).collect::<Vec<_>>();
    let provider = TestProvider::with_candidates(5, 0, candidates.clone());
    let store = BalStoreHandle::new(InMemoryBalStore::default());
    let client = ScriptedBalClient::default();
    for response_len in [2, 2, 1] {
        client.push_response(successful_response(vec![Some(raw.clone()); response_len]));
    }
    let config = HistoricalBalWorkerConfig::new(true, 1, 2, 2, 5).unwrap();
    let mut worker = test_worker(provider, store, client.clone(), config);

    worker.run_once().await;

    let requests = client.requests();
    assert_eq!(requests.iter().map(|(hashes, _)| hashes.len()).collect::<Vec<_>>(), [2, 2, 1]);
    assert!(requests.iter().all(|(_, requirement)| *requirement == BalRequirement::Optional));
    assert_eq!(client.max_active(), 2);
}

#[tokio::test]
async fn positive_short_prefix_continues_only_missing_tail_without_cooldown() {
    let raw = Bytes::from_static(&[0xc0]);
    let expected = keccak256(&raw);
    let candidates =
        (1..=3).map(|number| candidate(number, Some(expected), 1, true)).collect::<Vec<_>>();
    let provider = TestProvider::with_candidates(3, 0, candidates.clone());
    let store = BalStoreHandle::new(InMemoryBalStore::default());
    let client = ScriptedBalClient::default();
    client.push_response(successful_response(vec![Some(raw.clone())]));
    client.push_response(successful_response(vec![Some(raw.clone()), Some(raw.clone())]));
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let config = HistoricalBalWorkerConfig::new(true, 1, 3, 1, 3).unwrap();
    let mut worker = test_worker(provider, store.clone(), client.clone(), config);
    worker.metrics = metrics::with_local_recorder(&recorder, || {
        HistoricalBalDownloaderMetrics::new_with_labels(Vec::<metrics::Label>::new())
    });

    worker.run_once().await;

    assert_eq!(
        client.requests(),
        vec![
            (
                candidates.iter().map(|candidate| candidate.num_hash.hash).collect(),
                BalRequirement::Optional,
            ),
            (
                candidates[1..].iter().map(|candidate| candidate.num_hash.hash).collect(),
                BalRequirement::Optional,
            ),
        ]
    );
    for candidate in candidates {
        assert_eq!(store.get_by_hash(candidate.num_hash.hash).unwrap(), Some(raw.clone()));
    }
    let counters = counter_values(&snapshotter);
    assert_eq!(counters.get("downloaders.historical_bal.requested"), Some(&5));
    assert_eq!(counters.get("downloaders.historical_bal.downloaded"), Some(&3));
    assert_eq!(counters.get("downloaders.historical_bal.unavailable"), Some(&2));
}

#[tokio::test]
async fn short_prefix_none_stays_cooled_while_missing_tail_continues() {
    let raw = Bytes::from_static(&[0xc0]);
    let expected = keccak256(&raw);
    let candidates =
        (1..=4).map(|number| candidate(number, Some(expected), 1, true)).collect::<Vec<_>>();
    let provider = TestProvider::with_candidates(4, 0, candidates.clone());
    let store = BalStoreHandle::new(InMemoryBalStore::default());
    let client = ScriptedBalClient::default();
    client.push_response(successful_response(vec![Some(raw.clone()), None]));
    client.push_response(successful_response(vec![Some(raw.clone()), Some(raw.clone())]));
    let config = HistoricalBalWorkerConfig::new(true, 1, 4, 1, 4).unwrap();
    let mut worker = test_worker(provider, store.clone(), client.clone(), config);

    worker.run_once().await;
    worker.run_once().await;

    assert_eq!(
        client.requests(),
        vec![
            (
                candidates.iter().map(|candidate| candidate.num_hash.hash).collect(),
                BalRequirement::Optional,
            ),
            (
                candidates[2..].iter().map(|candidate| candidate.num_hash.hash).collect(),
                BalRequirement::Optional,
            ),
        ]
    );
    assert_eq!(store.get_by_hash(candidates[0].num_hash.hash).unwrap(), Some(raw.clone()));
    assert_eq!(store.get_by_hash(candidates[1].num_hash.hash).unwrap(), None);
    for candidate in &candidates[2..] {
        assert_eq!(store.get_by_hash(candidate.num_hash.hash).unwrap(), Some(raw.clone()));
    }
}

#[tokio::test]
async fn repeated_positive_prefixes_shrink_monotonically_and_stop_at_lookahead() {
    let raw = Bytes::from_static(&[0xc0]);
    let expected = keccak256(&raw);
    let candidates =
        (1..=4).map(|number| candidate(number, Some(expected), 1, true)).collect::<Vec<_>>();
    let provider = TestProvider::with_candidates(4, 0, candidates.clone());
    let store = BalStoreHandle::new(InMemoryBalStore::default());
    let client = ScriptedBalClient::default();
    for _ in &candidates {
        client.push_response(successful_response(vec![Some(raw.clone())]));
    }
    let config = HistoricalBalWorkerConfig::new(true, 1, 4, 1, 4).unwrap();
    let mut worker = test_worker(provider, store.clone(), client.clone(), config);

    worker.run_once().await;

    let expected_requests = (0..candidates.len())
        .map(|start| {
            (
                candidates[start..].iter().map(|candidate| candidate.num_hash.hash).collect(),
                BalRequirement::Optional,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(client.requests(), expected_requests);
    assert_eq!(client.requests().len(), candidates.len());
    assert_eq!(client.max_active(), 1);
    assert_eq!(worker.attempted.len(), candidates.len());
    for candidate in candidates {
        assert_eq!(store.get_by_hash(candidate.num_hash.hash).unwrap(), Some(raw.clone()));
    }
}

#[tokio::test]
async fn zero_progress_response_does_not_continue_or_bypass_cooldown() {
    let raw = Bytes::from_static(&[0xc0]);
    let expected = keccak256(&raw);
    let candidates =
        (1..=3).map(|number| candidate(number, Some(expected), 1, true)).collect::<Vec<_>>();
    let provider = TestProvider::with_candidates(3, 0, candidates.clone());
    let store = BalStoreHandle::new(InMemoryBalStore::default());
    let client = ScriptedBalClient::default();
    client.push_response(successful_response(Vec::new()));
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let config = HistoricalBalWorkerConfig::new(true, 1, 3, 1, 3).unwrap();
    let mut worker = test_worker(provider, store.clone(), client.clone(), config);
    worker.metrics = metrics::with_local_recorder(&recorder, || {
        HistoricalBalDownloaderMetrics::new_with_labels(Vec::<metrics::Label>::new())
    });

    worker.run_once().await;
    worker.run_once().await;

    assert_eq!(client.requests().len(), 1);
    assert_eq!(worker.attempted.len(), candidates.len());
    for candidate in candidates {
        assert_eq!(store.get_by_hash(candidate.num_hash.hash).unwrap(), None);
    }
    let counters = counter_values(&snapshotter);
    assert_eq!(counters.get("downloaders.historical_bal.requested"), Some(&3));
    assert_eq!(counters.get("downloaders.historical_bal.downloaded"), Some(&0));
    assert_eq!(counters.get("downloaders.historical_bal.unavailable"), Some(&3));
}

#[tokio::test]
async fn request_error_and_invalid_short_response_do_not_continue() {
    let raw = Bytes::from_static(&[0xc0]);
    let expected = keccak256(&raw);
    let candidates =
        (1..=3).map(|number| candidate(number, Some(expected), 1, true)).collect::<Vec<_>>();
    let provider = TestProvider::with_candidates(3, 0, candidates.clone());
    let client = ScriptedBalClient::default();
    client.push_response(Err(RequestError::Timeout));
    let config = HistoricalBalWorkerConfig::new(true, 1, 3, 1, 3).unwrap();
    let mut worker = test_worker(
        provider,
        BalStoreHandle::new(InMemoryBalStore::default()),
        client.clone(),
        config,
    );

    worker.run_once().await;
    worker.run_once().await;

    assert_eq!(client.requests().len(), 1);
    assert_eq!(worker.attempted.len(), candidates.len());
    assert!(client.reports().is_empty());

    let invalid_candidates =
        (1..=3).map(|number| candidate(number, Some(B256::ZERO), 1, true)).collect::<Vec<_>>();
    let provider = TestProvider::with_candidates(3, 0, invalid_candidates.clone());
    let store = BalStoreHandle::new(InMemoryBalStore::default());
    let client = ScriptedBalClient::default();
    let peer = PeerId::random();
    client.push_response(Ok(WithPeerId::new(peer, BlockAccessLists(vec![Some(raw)]))));
    let mut worker = test_worker(provider, store.clone(), client.clone(), config);

    worker.run_once().await;
    worker.run_once().await;

    assert_eq!(client.requests().len(), 1);
    assert_eq!(worker.attempted.len(), invalid_candidates.len());
    assert_eq!(client.reports(), vec![peer]);
    for candidate in invalid_candidates {
        assert_eq!(store.get_by_hash(candidate.num_hash.hash).unwrap(), None);
    }
}

#[tokio::test]
async fn short_tail_rejected_by_progress_stays_cooled_if_window_reopens() {
    let raw = Bytes::from_static(&[0xc0]);
    let expected = keccak256(&raw);
    let candidates =
        (1..=3).map(|number| candidate(number, Some(expected), 1, true)).collect::<Vec<_>>();
    let provider = TestProvider::with_candidates(3, 0, candidates.clone());
    provider.set_checkpoint_overrides([(3, 0), (3, 0), (3, 3)]);
    let store = BalStoreHandle::new(InMemoryBalStore::default());
    let client = ScriptedBalClient::default();
    client.push_response(successful_response(vec![Some(raw.clone())]));
    let config = HistoricalBalWorkerConfig::new(true, 1, 3, 1, 3).unwrap();
    let mut worker = test_worker(provider, store.clone(), client.clone(), config);

    worker.run_once().await;
    worker.run_once().await;

    assert_eq!(client.requests().len(), 1);
    assert_eq!(store.get_by_hash(candidates[0].num_hash.hash).unwrap(), Some(raw));
    for candidate in &candidates[1..] {
        assert_eq!(store.get_by_hash(candidate.num_hash.hash).unwrap(), None);
        assert!(worker.attempted.contains_key(&candidate.num_hash));
    }
}

#[tokio::test]
async fn completed_response_is_persisted_before_pending_tail_resolves() {
    let raw = Bytes::from_static(&[0xc0]);
    let expected = keccak256(&raw);
    let candidates =
        (1..=3).map(|number| candidate(number, Some(expected), 1, true)).collect::<Vec<_>>();
    let provider = TestProvider::with_candidates(3, 0, candidates.clone());
    let insert_notify = Arc::new(Notify::new());
    let store = BalStoreHandle::new(FaultingBalStore {
        insert_notify: Some(Arc::clone(&insert_notify)),
        ..Default::default()
    });
    let client = ScriptedBalClient::default();
    for _ in 0..2 {
        client.push_response(successful_response(vec![Some(raw.clone())]));
    }
    let pending = client.push_pending_response();
    let config = HistoricalBalWorkerConfig::new(true, 1, 1, 2, 3).unwrap();
    let mut worker = test_worker(provider, store.clone(), client, config);

    let task = tokio::spawn(async move { worker.run_once().await });
    let persisted_before_tail =
        tokio::time::timeout(Duration::from_secs(1), insert_notify.notified()).await.is_ok();
    pending.send(successful_response(vec![Some(raw.clone())])).unwrap();
    task.await.unwrap();

    assert!(persisted_before_tail);
    for candidate in candidates {
        assert_eq!(store.get_by_hash(candidate.num_hash.hash).unwrap(), Some(raw.clone()));
    }
}

#[tokio::test]
async fn blanket_provider_scan_filters_commitment_work_and_store_hits() {
    let raw = Bytes::from_static(&[0xc0]);
    let expected = keccak256(&raw);
    let provider = RangeMockProvider::new();
    add_mock_header(&provider, 1, None, 3);
    add_mock_header(&provider, 2, Some(expected), 1);
    let stored_hash = add_mock_header(&provider, 3, Some(expected), 3);
    let requested_hash = add_mock_header(&provider, 4, Some(expected), 2);
    provider.inner.add_stage_checkpoint(StageId::Bodies, StageCheckpoint::new(4));
    provider.inner.add_stage_checkpoint(StageId::Execution, StageCheckpoint::new(0));

    let recording_store = FaultingBalStore::default();
    let store = BalStoreHandle::new(recording_store.clone());
    store.insert(NumHash::new(3, stored_hash), RawBal::new(raw.clone())).unwrap();
    let client = ScriptedBalClient::default();
    client.push_response(successful_response(vec![Some(raw)]));
    let config = HistoricalBalWorkerConfig::new(true, 2, 4, 1, 4).unwrap();
    let mut worker = test_worker(provider, store, client.clone(), config);

    worker.run_once().await;

    assert_eq!(client.requests(), vec![(vec![requested_hash], BalRequirement::Optional)]);
    assert_eq!(
        recording_store.lookups(),
        vec![vec![stored_hash, requested_hash], vec![requested_hash]]
    );
    assert!(client.reports().is_empty());
}

#[test]
fn scan_rejects_missing_body_index_range_without_store_lookup() {
    let provider = RangeMockProvider::new();
    let raw = Bytes::from_static(&[0xc0]);
    add_mock_header(&provider, 1, Some(keccak256(raw.clone())), 1);
    add_mock_header(&provider, 2, Some(keccak256(raw.clone())), 1);
    add_mock_header(&provider, 3, Some(keccak256(raw)), 1);
    provider.remove_body_indices(2);
    let recording_store = FaultingBalStore::default();
    let store = BalStoreHandle::new(recording_store.clone());

    let result = provider.historical_bal_scan(
        1..=3,
        &store,
        BalExecutionPolicy::new(NonZeroU64::new(1).unwrap()),
    );

    assert!(matches!(
        result,
        Err(reth_storage_api::errors::provider::ProviderError::InvalidStorageOutput)
    ));
    assert!(recording_store.lookups().is_empty());
}

#[test]
fn scan_rejects_missing_header_range_without_store_lookup() {
    let provider = RangeMockProvider::new();
    let raw = Bytes::from_static(&[0xc0]);
    add_mock_header(&provider, 1, Some(keccak256(raw.clone())), 1);
    let missing = add_mock_header(&provider, 2, Some(keccak256(raw.clone())), 1);
    add_mock_header(&provider, 3, Some(keccak256(raw)), 1);
    provider.remove_header(missing);
    let recording_store = FaultingBalStore::default();
    let store = BalStoreHandle::new(recording_store.clone());

    let result = provider.historical_bal_scan(
        1..=3,
        &store,
        BalExecutionPolicy::new(NonZeroU64::new(1).unwrap()),
    );

    assert!(matches!(
        result,
        Err(reth_storage_api::errors::provider::ProviderError::InvalidStorageOutput)
    ));
    assert!(recording_store.lookups().is_empty());
}

#[test]
fn canonical_ranges_skip_candidate_gaps_and_reject_short_runs() {
    let provider = RangeMockProvider::new();
    let first = add_mock_header(&provider, 1, None, 1);
    let second = add_mock_header(&provider, 2, None, 1);
    let third = add_mock_header(&provider, 3, None, 1);
    let fourth = add_mock_header(&provider, 4, None, 1);
    let blocks = [NumHash::new(1, first), NumHash::new(3, third), NumHash::new(4, fourth)];

    provider.remove_header(second);
    assert_eq!(provider.historical_bal_canonical(&blocks).unwrap(), [true, true, true]);
    assert_eq!(provider.canonical_ranges(), [(1, 2), (3, 5)]);

    provider.remove_header(fourth);
    assert!(matches!(
        provider.historical_bal_canonical(&blocks),
        Err(reth_storage_api::errors::provider::ProviderError::InvalidStorageOutput)
    ));
}

#[test]
fn scan_rejects_misaligned_store_output() {
    let provider = RangeMockProvider::new();
    let raw = Bytes::from_static(&[0xc0]);
    add_mock_header(&provider, 1, Some(keccak256(raw)), 1);
    let recording_store = FaultingBalStore { truncate_lookup: true, ..Default::default() };
    let store = BalStoreHandle::new(recording_store);

    let result = provider.historical_bal_scan(
        1..=1,
        &store,
        BalExecutionPolicy::new(NonZeroU64::new(1).unwrap()),
    );

    assert!(matches!(
        result,
        Err(reth_storage_api::errors::provider::ProviderError::InvalidStorageOutput)
    ));
}

#[test]
fn worker_config_enforces_bounds_and_window() {
    let config = HistoricalBalWorkerConfig::new(true, 1, 2_048, 3, 64);
    assert_eq!(config.unwrap_err(), HistoricalBalConfigError::RequestBatchTooLarge(2_048));

    let config = HistoricalBalWorkerConfig::new(true, 1, 1, Semaphore::MAX_PERMITS + 1, 64);
    assert_eq!(
        config.unwrap_err(),
        HistoricalBalConfigError::ConcurrencyTooLarge(Semaphore::MAX_PERMITS + 1)
    );

    let invalid = reth_config::HistoricalBalConfig {
        request_batch_size: NonZeroUsize::new(2_048).unwrap(),
        ..Default::default()
    };
    assert_eq!(
        HistoricalBalWorkerConfig::try_from(invalid).unwrap_err(),
        HistoricalBalConfigError::RequestBatchTooLarge(2_048)
    );

    let config = HistoricalBalWorkerConfig::new(true, 1, 2, 3, 64).unwrap();
    assert_eq!(config.window(10, 100), Some(11..=74));
    assert_eq!(config.window(100, 100), None);
    assert_eq!(config.window(u64::MAX, u64::MAX), None);
}

#[test]
fn filters_commitment_work_store_window_and_attempts() {
    let now = Instant::now();
    let mut attempted = HashMap::from([(candidate(5, Some(B256::ZERO), 4, true).num_hash, now)]);
    let policy = BalExecutionPolicy::new(NonZeroU64::new(3).unwrap());
    let candidates = vec![
        candidate(1, None, 5, true),
        candidate(2, Some(B256::ZERO), 2, true),
        candidate(3, Some(B256::ZERO), 3, false),
        candidate(4, Some(B256::ZERO), 3, true),
        candidate(5, Some(B256::ZERO), 3, true),
        candidate(6, Some(B256::ZERO), 3, true),
    ];
    let result = filter_candidates(
        candidates,
        policy,
        &(3..=5),
        &mut attempted,
        now,
        Duration::from_secs(30),
    );
    assert_eq!(result.candidates.iter().map(|c| c.num_hash.number).collect::<Vec<_>>(), vec![4]);
    assert_eq!(result.skipped, 5);
}

#[test]
fn attempted_state_prunes_old_forks_and_allows_new_canonical_hash() {
    let now = Instant::now();
    let old = candidate(5, Some(B256::ZERO), 3, true);
    let current = ScannedCandidate { num_hash: NumHash::new(5, B256::repeat_byte(0x22)), ..old };
    let next = ScannedCandidate { num_hash: NumHash::new(5, B256::repeat_byte(0x33)), ..old };
    let mut attempted = HashMap::from([(old.num_hash, now), (current.num_hash, now)]);
    let policy = BalExecutionPolicy::new(NonZeroU64::new(1).unwrap());

    let result = filter_candidates(
        [current],
        policy,
        &(5..=5),
        &mut attempted,
        now,
        Duration::from_secs(30),
    );
    assert!(result.candidates.is_empty());
    assert_eq!(attempted.len(), 1);
    assert!(attempted.contains_key(&current.num_hash));

    let result =
        filter_candidates([next], policy, &(5..=5), &mut attempted, now, Duration::from_secs(30));
    assert_eq!(
        result.candidates,
        vec![EligibleCandidate { num_hash: next.num_hash, commitment: B256::ZERO }]
    );
    assert!(attempted.is_empty());
}

#[test]
fn attempted_cooldown_retries_at_boundary() {
    let attempted_at = Instant::now();
    let candidate = candidate(5, Some(B256::ZERO), 3, true);
    let mut attempted = HashMap::from([(candidate.num_hash, attempted_at)]);
    let policy = BalExecutionPolicy::new(NonZeroU64::new(1).unwrap());
    let cooldown = Duration::from_secs(30);

    let before_boundary = filter_candidates(
        [candidate],
        policy,
        &(5..=5),
        &mut attempted,
        attempted_at + cooldown - Duration::from_nanos(1),
        cooldown,
    );
    assert!(before_boundary.candidates.is_empty());

    let at_boundary = filter_candidates(
        [candidate],
        policy,
        &(5..=5),
        &mut attempted,
        attempted_at + cooldown,
        cooldown,
    );
    assert_eq!(
        at_boundary.candidates,
        vec![EligibleCandidate { num_hash: candidate.num_hash, commitment: B256::ZERO }]
    );
    assert!(attempted.is_empty());
}

#[test]
fn validates_prefix_none_short_tail_and_request_error_without_reporting() {
    let bal = Bytes::from_static(&[0xc0]);
    let expected = keccak256(&bal);
    let client = ScriptedBalClient::default();
    let requested = vec![eligible_candidate(1, expected), eligible_candidate(2, expected)];
    let peer = PeerId::random();

    let response = Ok(WithPeerId::new(peer, BlockAccessLists(vec![Some(bal.clone()), None])));
    let outcome = validate_bal_response(&client, &requested, response);
    assert_eq!(outcome.values.len(), 1);
    assert_eq!(outcome.unavailable, 1);
    assert_eq!(outcome.invalid, 0);

    let response = Ok(WithPeerId::new(peer, BlockAccessLists(vec![None, Some(bal.clone())])));
    let outcome = validate_bal_response(&client, &requested, response);
    assert_eq!(outcome.values.len(), 1);
    assert_eq!(outcome.values[0].0, requested[1].num_hash);
    assert_eq!(outcome.unavailable, 1);

    let response = Ok(WithPeerId::new(peer, BlockAccessLists(vec![Some(bal)])));
    let outcome = validate_bal_response(&client, &requested, response);
    assert_eq!(outcome.values.len(), 1);
    assert_eq!(outcome.unavailable, 1);

    let outcome = validate_bal_response(
        &client,
        &requested,
        Err(reth_network_p2p::error::RequestError::Timeout),
    );
    assert_eq!(outcome.unavailable, requested.len());
    assert!(client.reports().is_empty());
}

#[test]
fn rejects_overlong_response_and_reports_peer_once() {
    let client = ScriptedBalClient::default();
    let requested = [eligible_candidate(1, B256::ZERO)];
    let peer = PeerId::random();

    let outcome = validate_bal_response(
        &client,
        &requested,
        Ok(WithPeerId::new(peer, BlockAccessLists(vec![None, None]))),
    );

    assert!(outcome.values.is_empty());
    assert_eq!(outcome.unavailable, 0);
    assert_eq!(outcome.invalid, 1);
    assert_eq!(client.reports(), vec![peer]);
}

#[test]
fn rejects_malformed_nested_rlp_even_when_raw_hash_matches() {
    let malformed = Bytes::from_static(&[0xc1, 0x7f]);
    let client = ScriptedBalClient::default();
    let requested = [eligible_candidate(1, keccak256(&malformed))];
    let peer = PeerId::random();
    let encoded = alloy_rlp::encode(BlockAccessLists(vec![Some(malformed)]));
    let response = alloy_rlp::decode_exact::<BlockAccessLists>(&encoded).unwrap();

    let outcome = validate_bal_response(&client, &requested, Ok(WithPeerId::new(peer, response)));

    assert!(outcome.values.is_empty());
    assert_eq!(outcome.invalid, 1);
    assert_eq!(client.reports(), vec![peer]);
}

#[test]
fn rejects_hash_mismatch_once_and_preserves_valid_prefix() {
    let bal = Bytes::from_static(&[0xc0]);
    let client = ScriptedBalClient::default();
    let requested = [eligible_candidate(1, keccak256(&bal)), eligible_candidate(2, B256::ZERO)];
    let peer = PeerId::random();

    let outcome = validate_bal_response(
        &client,
        &requested,
        Ok(WithPeerId::new(peer, BlockAccessLists(vec![Some(bal.clone()), Some(bal)]))),
    );

    assert_eq!(outcome.values.len(), 1);
    assert_eq!(outcome.values[0].0, requested[0].num_hash);
    assert_eq!(outcome.invalid, 1);
    assert_eq!(client.reports(), vec![peer]);
}

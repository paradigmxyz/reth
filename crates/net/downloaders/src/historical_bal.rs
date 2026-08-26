//! Best-effort downloading of historical block access lists.
//!
//! Pipeline notifications are wakeups only. Every wake reconciles committed checkpoints and
//! canonical storage before it creates a network request.

use alloy_consensus::BlockHeader;
use alloy_eip7928::bal::{DecodedBal, RawBal};
use alloy_eips::NumHash;
use alloy_primitives::B256;
use futures::{stream::FuturesUnordered, Stream, StreamExt};
use reth_execution_types::BalExecutionPolicy;
use reth_network_p2p::{
    block_access_lists::client::{BalRequirement, BlockAccessListsClient},
    download::DownloadClient,
    error::PeerRequestResult,
    BlockAccessLists,
};
use reth_stages_types::StageId;
use reth_storage_api::{
    BalStoreHandle, BlockBodyIndicesProvider, BlockHashReader, HeaderProvider,
    StageCheckpointReader,
};
use reth_tasks::Runtime;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    num::{NonZeroU64, NonZeroUsize},
    ops::RangeInclusive,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;

use crate::metrics::HistoricalBalDownloaderMetrics;

/// The maximum number of hashes accepted by an eth/71 BAL request.
const MAX_REQUEST_BATCH_SIZE: usize = reth_config::HistoricalBalConfig::MAX_REQUEST_BATCH_SIZE;

/// A detached, best-effort historical BAL downloader.
#[derive(Debug)]
pub struct HistoricalBalWorker<P, C, W> {
    provider: P,
    store: BalStoreHandle,
    client: C,
    wake_stream: W,
    runtime: Runtime,
    config: HistoricalBalWorkerConfig,
    retry_cooldown: Duration,
    attempted: HashMap<NumHash, Instant>,
    pending_flush: HashSet<NumHash>,
    /// Identities passed to a flush that failed after the store may have marked them canonical.
    uncertain_flush: HashSet<NumHash>,
    metrics: HistoricalBalDownloaderMetrics,
}

/// Configuration consumed by [`HistoricalBalWorker`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoricalBalWorkerConfig {
    /// Whether the worker should issue requests.
    enabled: bool,
    /// Inclusive minimum transaction count.
    min_transactions: NonZeroU64,
    /// Maximum hashes in one request.
    request_batch_size: NonZeroUsize,
    /// Maximum number of requests in flight.
    max_concurrent_requests: NonZeroUsize,
    /// Maximum distance ahead of execution to inspect.
    lookahead: NonZeroU64,
}

/// Invalid worker configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HistoricalBalConfigError {
    /// A required positive value was zero.
    #[error("{0} must be greater than zero")]
    ZeroValue(&'static str),
    /// The request batch exceeded the protocol limit.
    #[error("request_batch_size {0} exceeds the eth/71 limit of {MAX_REQUEST_BATCH_SIZE}")]
    RequestBatchTooLarge(usize),
    /// The request concurrency exceeded the worker semaphore limit.
    #[error("max_concurrent_requests {0} exceeds the worker semaphore limit")]
    ConcurrencyTooLarge(usize),
}

impl HistoricalBalWorkerConfig {
    /// Creates a validated worker configuration.
    pub fn new(
        enabled: bool,
        min_transactions: u64,
        request_batch_size: usize,
        max_concurrent_requests: usize,
        lookahead: u64,
    ) -> Result<Self, HistoricalBalConfigError> {
        let min_transactions = NonZeroU64::new(min_transactions)
            .ok_or(HistoricalBalConfigError::ZeroValue("min_transactions"))?;
        let request_batch_size = NonZeroUsize::new(request_batch_size)
            .ok_or(HistoricalBalConfigError::ZeroValue("request_batch_size"))?;
        if request_batch_size.get() > MAX_REQUEST_BATCH_SIZE {
            return Err(HistoricalBalConfigError::RequestBatchTooLarge(request_batch_size.get()))
        }
        let max_concurrent_requests = NonZeroUsize::new(max_concurrent_requests)
            .ok_or(HistoricalBalConfigError::ZeroValue("max_concurrent_requests"))?;
        if max_concurrent_requests.get() > Semaphore::MAX_PERMITS {
            return Err(HistoricalBalConfigError::ConcurrencyTooLarge(max_concurrent_requests.get()))
        }
        let lookahead =
            NonZeroU64::new(lookahead).ok_or(HistoricalBalConfigError::ZeroValue("lookahead"))?;
        Ok(Self {
            enabled,
            min_transactions,
            request_batch_size,
            max_concurrent_requests,
            lookahead,
        })
    }

    /// Clamps lookahead to a caller-provided effective BAL retention distance.
    pub fn with_effective_lookahead(mut self, retention: NonZeroU64) -> Self {
        self.lookahead = NonZeroU64::new(self.lookahead.get().min(retention.get())).unwrap();
        self
    }

    /// Returns the inclusive historical execution window.
    fn window(&self, execution: u64, bodies: u64) -> Option<RangeInclusive<u64>> {
        let start = execution.checked_add(1)?;
        let end = bodies.min(execution.saturating_add(self.lookahead.get()));
        (start <= end).then_some(start..=end)
    }

    /// Returns the execution eligibility policy shared with execution consumers.
    const fn policy(&self) -> BalExecutionPolicy {
        BalExecutionPolicy::new(self.min_transactions)
    }
}

impl TryFrom<reth_config::HistoricalBalConfig> for HistoricalBalWorkerConfig {
    type Error = HistoricalBalConfigError;

    fn try_from(value: reth_config::HistoricalBalConfig) -> Result<Self, Self::Error> {
        Self::new(
            value.enabled,
            value.min_transactions.get(),
            value.request_batch_size.get(),
            value.max_concurrent_requests.get(),
            value.lookahead.get(),
        )
    }
}

/// A canonical block observed while scanning historical storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ScannedCandidate {
    num_hash: NumHash,
    commitment: Option<B256>,
    transaction_count: u64,
    store_miss: bool,
}

/// A scanned candidate whose structural request requirements have been validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct EligibleCandidate {
    num_hash: NumHash,
    commitment: B256,
}

/// The result of scanning one committed historical BAL window.
#[derive(Debug, Default, PartialEq, Eq)]
struct HistoricalBalScanOutcome {
    candidates: Vec<ScannedCandidate>,
    skipped: usize,
}

/// The result of applying worker eligibility filters.
#[derive(Debug, Default, PartialEq, Eq)]
struct HistoricalBalFilterOutcome {
    candidates: Vec<EligibleCandidate>,
    skipped: usize,
}

#[derive(Debug)]
struct FlushCandidates {
    canonical: Vec<NumHash>,
    stale: Vec<NumHash>,
}

/// Filters candidates in the locked order: commitment, work policy, store miss, window, attempts.
fn filter_candidates(
    candidates: impl IntoIterator<Item = ScannedCandidate>,
    policy: BalExecutionPolicy,
    window: &RangeInclusive<u64>,
    attempted: &mut HashMap<NumHash, Instant>,
    now: Instant,
    retry_cooldown: Duration,
) -> HistoricalBalFilterOutcome {
    let candidates = candidates.into_iter().collect::<Vec<_>>();
    let current = candidates.iter().map(|candidate| candidate.num_hash).collect::<HashSet<_>>();
    attempted.retain(|block, at| {
        current.contains(block) &&
            window.contains(&block.number) &&
            now.duration_since(*at) < retry_cooldown
    });

    let mut outcome = HistoricalBalFilterOutcome::default();
    for candidate in candidates {
        let Some(commitment) = candidate.commitment else {
            outcome.skipped += 1;
            continue
        };
        if !policy.is_eligible(candidate.transaction_count) ||
            !candidate.store_miss ||
            !window.contains(&candidate.num_hash.number) ||
            attempted.contains_key(&candidate.num_hash)
        {
            outcome.skipped += 1;
            continue
        }
        outcome.candidates.push(EligibleCandidate { num_hash: candidate.num_hash, commitment });
    }
    outcome
}

/// Validated BALs from one positional response.
#[derive(Debug, Default, PartialEq, Eq)]
struct BalResponseOutcome {
    values: Vec<(NumHash, RawBal)>,
    unavailable: usize,
    invalid: usize,
}

/// Validates one successful or failed positional BAL response.
fn validate_bal_response<C: DownloadClient>(
    client: &C,
    requested: &[EligibleCandidate],
    response: PeerRequestResult<BlockAccessLists>,
) -> BalResponseOutcome {
    let mut outcome = BalResponseOutcome::default();
    let Ok(response) = response else {
        outcome.unavailable = requested.len();
        return outcome
    };

    let (peer, response) = response.split();
    if response.0.len() > requested.len() {
        client.report_bad_message(peer);
        outcome.invalid = 1;
        return outcome
    }

    for (index, candidate) in requested.iter().enumerate() {
        let Some(entry) = response.0.get(index) else {
            outcome.unavailable += requested.len() - index;
            break
        };
        let Some(bytes) = entry else {
            outcome.unavailable += 1;
            continue
        };

        let raw = RawBal::new(bytes.clone());
        if raw.ensure_hash(candidate.commitment).is_err() ||
            DecodedBal::from_raw_bal(raw.clone()).is_err()
        {
            client.report_bad_message(peer);
            outcome.invalid = 1;
            break
        }
        outcome.values.push((candidate.num_hash, raw));
    }
    outcome
}

fn range_len(
    range: &RangeInclusive<u64>,
) -> reth_storage_api::errors::provider::ProviderResult<usize> {
    let len = range
        .end()
        .checked_sub(*range.start())
        .and_then(|len| len.checked_add(1))
        .ok_or(reth_storage_api::errors::provider::ProviderError::InvalidStorageOutput)?;
    usize::try_from(len)
        .map_err(|_| reth_storage_api::errors::provider::ProviderError::InvalidStorageOutput)
}

/// Provider operations required by the worker.
trait HistoricalBalProvider: Clone + Send + Sync + 'static {
    /// Reads the committed Bodies and Execution checkpoints.
    fn historical_bal_checkpoints(
        &self,
    ) -> reth_storage_api::errors::provider::ProviderResult<(u64, u64)>;
    /// Scans canonical headers, body indices, and BAL-store misses.
    fn historical_bal_scan(
        &self,
        range: RangeInclusive<u64>,
        store: &BalStoreHandle,
        policy: BalExecutionPolicy,
    ) -> reth_storage_api::errors::provider::ProviderResult<HistoricalBalScanOutcome>;
    /// Rechecks canonical hashes for the supplied blocks.
    fn historical_bal_canonical(
        &self,
        blocks: &[NumHash],
    ) -> reth_storage_api::errors::provider::ProviderResult<Vec<bool>>;
}

impl<P> HistoricalBalProvider for P
where
    P: HeaderProvider
        + BlockBodyIndicesProvider
        + BlockHashReader
        + StageCheckpointReader
        + Clone
        + Send
        + Sync
        + 'static,
{
    fn historical_bal_checkpoints(
        &self,
    ) -> reth_storage_api::errors::provider::ProviderResult<(u64, u64)> {
        let bodies = self
            .get_stage_checkpoint(StageId::Bodies)?
            .map_or(0, |checkpoint| checkpoint.block_number);
        let execution = self
            .get_stage_checkpoint(StageId::Execution)?
            .map_or(0, |checkpoint| checkpoint.block_number);
        Ok((bodies, execution))
    }

    fn historical_bal_scan(
        &self,
        range: RangeInclusive<u64>,
        store: &BalStoreHandle,
        policy: BalExecutionPolicy,
    ) -> reth_storage_api::errors::provider::ProviderResult<HistoricalBalScanOutcome> {
        let expected_len = range_len(&range)?;
        let headers = self.sealed_headers_range(range.clone())?;
        if headers.len() != expected_len {
            return Err(reth_storage_api::errors::provider::ProviderError::InvalidStorageOutput)
        }
        let start = *range.start();
        let indices = self.block_body_indices_range(range)?;
        if indices.len() != expected_len {
            return Err(reth_storage_api::errors::provider::ProviderError::InvalidStorageOutput)
        }

        let mut outcome =
            HistoricalBalScanOutcome { candidates: Vec::with_capacity(expected_len), skipped: 0 };
        // Range providers return ascending rows. Exact lengths and header numbers prevent a
        // missing row from silently shifting the positional body-index pairing.
        for (offset, (header, indices)) in headers.into_iter().zip(indices).enumerate() {
            let expected_number = start
                .checked_add(offset as u64)
                .ok_or(reth_storage_api::errors::provider::ProviderError::InvalidStorageOutput)?;
            if header.number() != expected_number {
                return Err(reth_storage_api::errors::provider::ProviderError::InvalidStorageOutput)
            }
            let commitment = header.header().block_access_list_hash();
            if commitment.is_none() || !policy.is_eligible(indices.tx_count) {
                outcome.skipped += 1;
                continue
            }
            outcome.candidates.push(ScannedCandidate {
                num_hash: NumHash::new(header.number(), header.hash()),
                commitment,
                transaction_count: indices.tx_count,
                store_miss: true,
            });
        }

        if outcome.candidates.is_empty() {
            return Ok(outcome)
        }
        let hashes =
            outcome.candidates.iter().map(|candidate| candidate.num_hash.hash).collect::<Vec<_>>();
        let stored = store.get_by_hashes(&hashes)?;
        if stored.len() != hashes.len() {
            return Err(reth_storage_api::errors::provider::ProviderError::InvalidStorageOutput)
        }
        for (candidate, stored) in outcome.candidates.iter_mut().zip(stored) {
            candidate.store_miss = stored.is_none();
        }
        Ok(outcome)
    }

    fn historical_bal_canonical(
        &self,
        blocks: &[NumHash],
    ) -> reth_storage_api::errors::provider::ProviderResult<Vec<bool>> {
        let mut canonical = Vec::with_capacity(blocks.len());
        for run in blocks.chunk_by(|left, right| left.number.checked_add(1) == Some(right.number)) {
            let start = run[0].number;
            let end = run[run.len() - 1]
                .number
                .checked_add(1)
                .ok_or(reth_storage_api::errors::provider::ProviderError::InvalidStorageOutput)?;
            let hashes = self.canonical_hashes_range(start, end)?;
            if hashes.len() != run.len() {
                return Err(reth_storage_api::errors::provider::ProviderError::InvalidStorageOutput)
            }
            canonical.extend(run.iter().zip(hashes).map(|(block, hash)| block.hash == hash));
        }
        Ok(canonical)
    }
}

impl<P, C, W> HistoricalBalWorker<P, C, W> {
    /// Creates a worker. `config.lookahead` should already be clamped to store retention.
    pub fn new(
        provider: P,
        store: BalStoreHandle,
        client: C,
        wake_stream: W,
        runtime: Runtime,
        config: HistoricalBalWorkerConfig,
    ) -> Self
    where
        P: HeaderProvider
            + BlockBodyIndicesProvider
            + BlockHashReader
            + StageCheckpointReader
            + Clone
            + Send
            + Sync
            + 'static,
        C: BlockAccessListsClient + Clone + 'static,
        C::Output: 'static,
        W: Stream + Send + Unpin + 'static,
        W::Item: Send,
    {
        Self::from_parts(provider, store, client, wake_stream, runtime, config)
    }

    fn from_parts(
        provider: P,
        store: BalStoreHandle,
        client: C,
        wake_stream: W,
        runtime: Runtime,
        config: HistoricalBalWorkerConfig,
    ) -> Self {
        Self {
            provider,
            store,
            client,
            wake_stream,
            runtime,
            config,
            retry_cooldown: Duration::from_secs(30),
            attempted: HashMap::new(),
            pending_flush: HashSet::new(),
            uncertain_flush: HashSet::new(),
            metrics: HistoricalBalDownloaderMetrics::default(),
        }
    }

    /// Spawns this worker as a regular detached task.
    pub fn spawn(self) -> tokio::task::JoinHandle<()>
    where
        P: HeaderProvider
            + BlockBodyIndicesProvider
            + BlockHashReader
            + StageCheckpointReader
            + Clone
            + Send
            + Sync
            + 'static,
        C: BlockAccessListsClient + Clone + 'static,
        C::Output: 'static,
        W: Stream + Send + Unpin + 'static,
        W::Item: Send,
    {
        let runtime = self.runtime.clone();
        runtime.spawn_task(self.run())
    }

    async fn run(mut self)
    where
        P: HistoricalBalProvider,
        C: BlockAccessListsClient + Clone + 'static,
        C::Output: 'static,
        W: Stream + Send + Unpin + 'static,
        W::Item: Send,
    {
        self.run_once().await;
        while self.wake_stream.next().await.is_some() {
            self.run_once().await;
        }
    }

    async fn run_once(&mut self)
    where
        P: HistoricalBalProvider,
        C: BlockAccessListsClient + Clone + 'static,
        C::Output: 'static,
    {
        if !self.config.enabled {
            return
        }

        self.retry_pending_flush().await;

        let provider = self.provider.clone();
        let Ok(Ok((bodies, execution))) =
            self.runtime.spawn_blocking(move || provider.historical_bal_checkpoints()).await
        else {
            return
        };
        let Some(window) = self.config.window(execution, bodies) else {
            self.attempted.clear();
            return
        };

        let provider = self.provider.clone();
        let store = self.store.clone();
        let scan_range = window.clone();
        let policy = self.config.policy();
        let Ok(Ok(scan)) = self
            .runtime
            .spawn_blocking(move || provider.historical_bal_scan(scan_range, &store, policy))
            .await
        else {
            return
        };

        self.metrics.skipped.increment(scan.skipped as u64);

        let now = Instant::now();
        let mut filtered = filter_candidates(
            scan.candidates,
            self.config.policy(),
            &window,
            &mut self.attempted,
            now,
            self.retry_cooldown,
        );
        self.metrics.skipped.increment(filtered.skipped as u64);
        let pending_flush_limit =
            usize::try_from(self.config.lookahead.get()).unwrap_or(usize::MAX);
        let available = pending_flush_limit.saturating_sub(self.pending_flush.len());
        if filtered.candidates.len() > available {
            self.metrics.skipped.increment((filtered.candidates.len() - available) as u64);
            // Failed flushes consume admission capacity. Evicting them would strand buffered BALs
            // once the store reports a hit, so new work waits for retry capacity instead.
            filtered.candidates.truncate(available);
        }
        if filtered.candidates.is_empty() {
            return
        }

        let max_concurrent_requests = self.config.max_concurrent_requests.get();
        let semaphore = Arc::new(Semaphore::new(max_concurrent_requests));
        let mut in_flight = FuturesUnordered::new();
        let request_batch_size = self.config.request_batch_size.get();
        let mut pending = filtered
            .candidates
            .chunks(request_batch_size)
            .map(|chunk| chunk.to_vec())
            .collect::<VecDeque<_>>();
        let mut scheduling = true;

        while !pending.is_empty() || !in_flight.is_empty() {
            while scheduling && in_flight.len() < max_concurrent_requests {
                let Some(chunk) = pending.pop_front() else { break };
                let permit = semaphore
                    .clone()
                    .acquire_owned()
                    .await
                    .expect("the reconciliation-local request semaphore is never closed");

                let provider = self.provider.clone();
                let store = self.store.clone();
                let config = self.config;
                let original_len = chunk.len();
                let dispatchable = match self
                    .runtime
                    .spawn_blocking(move || {
                        let (bodies, execution) = provider.historical_bal_checkpoints()?;
                        let Some(window) = config.window(execution, bodies) else {
                            return Ok(Vec::new())
                        };
                        let blocks =
                            chunk.iter().map(|candidate| candidate.num_hash).collect::<Vec<_>>();
                        let hashes = blocks.iter().map(|block| block.hash).collect::<Vec<_>>();
                        let canonical = provider.historical_bal_canonical(&blocks)?;
                        let stored = store.get_by_hashes(&hashes)?;
                        if canonical.len() != chunk.len() || stored.len() != chunk.len() {
                            return Err(
                                reth_storage_api::errors::provider::ProviderError::InvalidStorageOutput,
                            )
                        }
                        Ok(chunk
                            .into_iter()
                            .zip(canonical)
                            .zip(stored)
                            .filter_map(|((candidate, canonical), stored)| {
                                (canonical &&
                                    stored.is_none() &&
                                    window.contains(&candidate.num_hash.number))
                                .then_some(candidate)
                            })
                            .collect::<Vec<_>>())
                    })
                    .await
                {
                    Ok(Ok(dispatchable)) => dispatchable,
                    _ => {
                        let abandoned =
                            original_len + pending.iter().map(Vec::len).sum::<usize>();
                        self.metrics.skipped.increment(abandoned as u64);
                        pending.clear();
                        scheduling = false;
                        drop(permit);
                        break
                    }
                };
                self.metrics.skipped.increment((original_len - dispatchable.len()) as u64);
                if dispatchable.is_empty() {
                    drop(permit);
                    continue
                }

                let attempted_at = Instant::now();
                self.attempted.extend(
                    dispatchable.iter().map(|candidate| (candidate.num_hash, attempted_at)),
                );
                let client = self.client.clone();
                let chunk = dispatchable;
                let hashes =
                    chunk.iter().map(|candidate| candidate.num_hash.hash).collect::<Vec<_>>();
                self.metrics.requested.increment(hashes.len() as u64);
                let response = client
                    .get_block_access_lists_with_requirement(hashes, BalRequirement::Optional);
                in_flight.push(async move {
                    let response = response.await;
                    drop(permit);
                    (client, chunk, response)
                });
            }

            let Some((client, requested, response)) = in_flight.next().await else { break };
            let continuation = self.process_response(client, requested, response).await;
            if !continuation.is_empty() {
                // Continuations bypass cooldown inside this reconciliation. Retaining the original
                // attempt keeps a suffix cooled if its pre-dispatch recheck rejects it; a suffix
                // that is actually dispatched refreshes the attempt timestamp above.
                pending.push_front(continuation);
            }
        }
    }

    async fn process_response(
        &mut self,
        client: C,
        requested: Vec<EligibleCandidate>,
        response: PeerRequestResult<BlockAccessLists>,
    ) -> Vec<EligibleCandidate>
    where
        P: HistoricalBalProvider,
        C: BlockAccessListsClient + Clone + 'static,
        C::Output: 'static,
    {
        let requested_len = requested.len();
        let mut continuation = response.as_ref().ok().and_then(|response| {
            let response_len = response.data().0.len();
            (response_len > 0 && response_len < requested_len)
                .then(|| requested[response_len..].to_vec())
        });
        let Ok(outcome) = self
            .runtime
            .spawn_blocking(move || validate_bal_response(&client, &requested, response))
            .await
        else {
            self.metrics.unavailable.increment(requested_len as u64);
            return Vec::new()
        };
        self.metrics.downloaded.increment(outcome.values.len() as u64);
        self.metrics.unavailable.increment(outcome.unavailable as u64);
        self.metrics.invalid.increment(outcome.invalid as u64);

        if outcome.invalid > 0 {
            continuation = None;
        }
        self.persist_valid(outcome.values).await;
        continuation.unwrap_or_default()
    }

    async fn persist_valid(&mut self, valid: Vec<(NumHash, RawBal)>)
    where
        P: HistoricalBalProvider,
    {
        if valid.is_empty() {
            return
        }

        let valid_blocks = valid.iter().map(|(num_hash, _)| *num_hash).collect::<Vec<_>>();
        let uncertain_blocks = valid_blocks.clone();
        let provider = self.provider.clone();
        let store = self.store.clone();
        let attempted_insert = self
            .runtime
            .spawn_blocking(
                move || -> reth_storage_api::errors::provider::ProviderResult<Vec<NumHash>> {
                    let canonical = provider.historical_bal_canonical(&valid_blocks)?;
                    if canonical.len() != valid.len() {
                        return Err(
                            reth_storage_api::errors::provider::ProviderError::InvalidStorageOutput,
                        )
                    }
                    let entries = valid
                        .into_iter()
                        .zip(canonical)
                        .filter_map(|(entry, canonical)| canonical.then_some(entry))
                        .collect::<Vec<_>>();
                    if entries.is_empty() {
                        return Ok(Vec::new())
                    }
                    let blocks = entries.iter().map(|(num_hash, _)| *num_hash).collect::<Vec<_>>();
                    if let Err(error) = store.insert_many(entries) {
                        tracing::debug!(
                            target: "downloaders::historical_bal",
                            %error,
                            blocks = blocks.len(),
                            "Historical BAL insertion may have partially succeeded"
                        );
                    }
                    Ok(blocks)
                },
            )
            .await;

        let blocks = match attempted_insert {
            Ok(Ok(blocks)) => blocks,
            Ok(Err(error)) => {
                tracing::debug!(
                    target: "downloaders::historical_bal",
                    %error,
                    "Failed to verify historical BAL canonicality before insertion"
                );
                return
            }
            Err(error) => {
                tracing::debug!(
                    target: "downloaders::historical_bal",
                    %error,
                    "Historical BAL insertion task failed with an unknown partial result"
                );
                uncertain_blocks
            }
        };
        if blocks.is_empty() {
            return
        }

        self.pending_flush.extend(blocks);
        self.retry_pending_flush().await;
    }

    async fn retry_pending_flush(&mut self)
    where
        P: HistoricalBalProvider,
    {
        if self.pending_flush.is_empty() {
            return
        }

        let mut blocks = self.pending_flush.iter().copied().collect::<Vec<_>>();
        blocks.sort_unstable_by_key(|block| block.number);
        let pending = blocks.len();
        let provider = self.provider.clone();
        let canonical_result = self
            .runtime
            .spawn_blocking(
                move || -> reth_storage_api::errors::provider::ProviderResult<FlushCandidates> {
                    let canonical = provider.historical_bal_canonical(&blocks)?;
                    if canonical.len() != blocks.len() {
                        return Err(
                            reth_storage_api::errors::provider::ProviderError::InvalidStorageOutput,
                        )
                    }
                    let (canonical_blocks, stale): (Vec<_>, Vec<_>) =
                        blocks.into_iter().zip(canonical).partition(|(_, canonical)| *canonical);
                    let canonical_blocks =
                        canonical_blocks.into_iter().map(|(block, _)| block).collect::<Vec<_>>();
                    let stale = stale.into_iter().map(|(block, _)| block).collect::<Vec<_>>();
                    Ok(FlushCandidates { canonical: canonical_blocks, stale })
                },
            )
            .await;

        let FlushCandidates { canonical, stale } = match canonical_result {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => {
                tracing::debug!(target: "downloaders::historical_bal", %error, pending,
                    "Failed to recheck pending historical BAL canonicality");
                return
            }
            Err(error) => {
                tracing::debug!(target: "downloaders::historical_bal", %error, pending,
                    "Historical BAL canonicality task failed");
                return
            }
        };

        // A failed flush may retain its internal canonical set. Do not issue a narrower retry while
        // any identity from that uncertain set is no longer canonical.
        if self.uncertain_flush.iter().any(|block| stale.contains(block)) {
            let uncertain_flush = &self.uncertain_flush;
            self.pending_flush
                .retain(|block| uncertain_flush.contains(block) || !stale.contains(block));
            self.uncertain_flush.retain(|block| self.pending_flush.contains(block));
            return
        }

        if canonical.is_empty() {
            self.pending_flush.retain(|block| !stale.contains(block));
            self.uncertain_flush.retain(|block| self.pending_flush.contains(block));
            return
        }

        let attempted = canonical.clone();
        let store = self.store.clone();
        let flush_result = self.runtime.spawn_blocking(move || store.flush(&canonical)).await;
        match flush_result {
            Ok(Ok(())) => {
                self.pending_flush
                    .retain(|block| !attempted.contains(block) && !stale.contains(block));
            }
            Ok(Err(error)) => {
                self.pending_flush.retain(|block| !stale.contains(block));
                self.uncertain_flush.extend(attempted);
                tracing::debug!(
                    target: "downloaders::historical_bal",
                    %error,
                    pending,
                    "Failed to flush pending historical BALs"
                );
            }
            Err(error) => {
                self.pending_flush.retain(|block| !stale.contains(block));
                self.uncertain_flush.extend(attempted);
                tracing::debug!(
                    target: "downloaders::historical_bal",
                    %error,
                    pending,
                    "Historical BAL flush task failed"
                );
            }
        }
        self.uncertain_flush.retain(|block| self.pending_flush.contains(block));
    }
}

#[cfg(test)]
mod tests;

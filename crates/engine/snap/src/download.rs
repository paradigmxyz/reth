//! Snap sync download loops for accounts, storage, and bytecodes.
//!
//! The main entry point is [`download_state`] which streams through the entire
//! state trie in account-hash order. For each batch of accounts it immediately
//! fetches the associated storage slots and bytecodes before moving on to the
//! next range. This keeps memory usage bounded to a single batch at a time
//! regardless of total state size.

use crate::{
    proof::verify_range_proof,
    storage::{increment_b256, write_bytecodes, write_hashed_accounts, write_hashed_storages},
    SnapSyncError, SNAP_RESPONSE_BYTES_LIMIT,
};
use alloy_consensus::EMPTY_ROOT_HASH;
use alloy_primitives::{keccak256, Bytes, B256, U256};
use futures::{stream::FuturesUnordered, StreamExt};
use reth_db_api::transaction::DbTxMut;
use reth_eth_wire_types::snap::{
    GetAccountRangeMessage, GetByteCodesMessage, GetStorageRangesMessage, StorageData, TrieAccount,
};
use reth_network_p2p::snap::client::{SnapClient, SnapResponse};
use reth_primitives_traits::Account;
use reth_provider::{DatabaseProviderFactory, HeaderProvider};
use reth_storage_api::{DBProvider, StateWriter};
use reth_trie::root::storage_root;
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use tokio::{
    sync::{mpsc, Semaphore},
    task::JoinSet,
};
use tracing::info;

/// Number of B256-space sub-ranges the state is split into for download.
/// More chunks improve load balancing (fast workers pick up new ranges as
/// they finish) and reduce tail latency from heavy ranges.
pub const PARALLEL_CHUNKS: usize = 64;

/// Maximum number of chunk tasks running concurrently. Bounds the load on
/// the serving peer regardless of [`PARALLEL_CHUNKS`].
const MAX_CONCURRENT_CHUNKS: usize = 16;

/// Maximum number of account hashes per storage range request.
const STORAGE_BATCH_SIZE: usize = 20;

/// Maximum number of in-flight storage range requests per chunk. Each batch
/// returns up to `STORAGE_BATCH_SIZE` accounts; running them concurrently
/// hides per-request RTT and verification latency.
const STORAGE_PARALLELISM: usize = 4;

/// Maximum number of code hashes per bytecode request.
const BYTECODE_BATCH_SIZE: usize = 50;

/// Threshold (in buffered items) at which a chunk hands off its accumulated
/// writes to the global writer task. Smaller values keep memory bounded;
/// the writer accumulates further before the actual MDBX commit.
const CHUNK_HANDOFF_THRESHOLD: usize = 50_000;

/// Threshold (in buffered items) at which the global writer task issues a
/// single MDBX commit. Larger values amortize fsync/transaction overhead.
const WRITER_FLUSH_THRESHOLD: usize = 500_000;

/// Capacity of the channel between chunk tasks and the global writer task.
/// Each message holds a chunk's accumulated batch, so this is the number of
/// such batches that can be queued before chunks block on send.
const WRITER_CHANNEL_CAPACITY: usize = 64;

/// Maximum hash value used as the range upper bound.
const MAX_HASH: B256 = B256::new([0xff; 32]);

/// Write batch passed from a chunk task to the global writer task.
/// Each chunk accumulates a batch locally and hands it off when full, so the
/// writer can merge writes from all chunks into single MDBX transactions.
struct PendingWrites {
    accounts: Vec<(B256, Account)>,
    storages: Vec<(B256, B256, U256)>,
    bytecodes: Vec<(B256, Bytes)>,
}

impl PendingWrites {
    fn new() -> Self {
        Self { accounts: Vec::new(), storages: Vec::new(), bytecodes: Vec::new() }
    }

    fn buffered_items(&self) -> usize {
        self.accounts.len() + self.storages.len() + self.bytecodes.len()
    }

    fn merge(&mut self, mut other: Self) {
        self.accounts.append(&mut other.accounts);
        self.storages.append(&mut other.storages);
        self.bytecodes.append(&mut other.bytecodes);
    }

    fn flush<F>(&mut self, factory: &F) -> Result<(), SnapSyncError>
    where
        F: DatabaseProviderFactory + Clone + Send + Sync + 'static,
        F::ProviderRW: DBProvider + StateWriter,
        <F::ProviderRW as DBProvider>::Tx: DbTxMut,
    {
        if !self.accounts.is_empty() {
            write_hashed_accounts(factory, &self.accounts)?;
            self.accounts.clear();
        }
        if !self.storages.is_empty() {
            write_hashed_storages(factory, &self.storages)?;
            self.storages.clear();
        }
        if !self.bytecodes.is_empty() {
            write_bytecodes(factory, &self.bytecodes)?;
            self.bytecodes.clear();
        }
        Ok(())
    }
}

/// Spawns a single writer task that drains [`PendingWrites`] from all chunk
/// tasks via an mpsc channel and commits them to MDBX in batched transactions.
/// Centralizing writes eliminates writer-lock contention between chunks.
fn spawn_writer_task<F>(
    factory: F,
) -> (mpsc::Sender<PendingWrites>, tokio::task::JoinHandle<Result<(), SnapSyncError>>)
where
    F: DatabaseProviderFactory + Clone + Send + Sync + 'static,
    F::ProviderRW: DBProvider + StateWriter,
    <F::ProviderRW as DBProvider>::Tx: DbTxMut,
{
    let (tx, mut rx) = mpsc::channel::<PendingWrites>(WRITER_CHANNEL_CAPACITY);
    let handle = tokio::task::spawn_blocking(move || -> Result<(), SnapSyncError> {
        let mut accumulator = PendingWrites::new();
        while let Some(batch) = rx.blocking_recv() {
            accumulator.merge(batch);
            if accumulator.buffered_items() >= WRITER_FLUSH_THRESHOLD {
                accumulator.flush(&factory)?;
            }
        }
        // Drain remaining buffered items on channel close.
        if accumulator.buffered_items() > 0 {
            accumulator.flush(&factory)?;
        }
        Ok(())
    });
    (tx, handle)
}

/// Hands `pending` off to the writer task if it has reached the chunk
/// handoff threshold. Replaces `pending` with an empty buffer.
async fn maybe_handoff(
    pending: &mut PendingWrites,
    writer: &mpsc::Sender<PendingWrites>,
) -> Result<(), SnapSyncError> {
    if pending.buffered_items() < CHUNK_HANDOFF_THRESHOLD {
        return Ok(());
    }
    handoff(pending, writer).await
}

/// Unconditionally hands `pending` off to the writer task.
async fn handoff(
    pending: &mut PendingWrites,
    writer: &mpsc::Sender<PendingWrites>,
) -> Result<(), SnapSyncError> {
    let batch = std::mem::replace(pending, PendingWrites::new());
    writer
        .send(batch)
        .await
        .map_err(|e| SnapSyncError::Database(format!("snap writer task closed: {e}")))
}

/// Returns the per-chunk lower bound (initial starting hash) for chunk `i`.
/// Chunks share the leading two bytes evenly across the 16-bit prefix space
/// so [`PARALLEL_CHUNKS`] is not constrained to powers of two ≤ 256.
fn chunk_lower(i: usize) -> B256 {
    let mut bytes = [0u8; 32];
    let prefix = ((i as u64) * 0x10000 / PARALLEL_CHUNKS as u64) as u16;
    bytes[0..2].copy_from_slice(&prefix.to_be_bytes());
    B256::from(bytes)
}

/// Returns the exclusive upper bound for chunk `i`. Chunk `PARALLEL_CHUNKS - 1`
/// uses [`MAX_HASH`] as the (effectively inclusive) terminator.
fn chunk_upper(i: usize) -> B256 {
    if i + 1 == PARALLEL_CHUNKS {
        return MAX_HASH;
    }
    let mut bytes = [0u8; 32];
    let prefix = (((i + 1) as u64) * 0x10000 / PARALLEL_CHUNKS as u64) as u16;
    bytes[0..2].copy_from_slice(&prefix.to_be_bytes());
    B256::from(bytes)
}

/// Returns the initial cursor array for a fresh parallel download
/// (`[chunk_lower(0), chunk_lower(1), …]`).
pub fn initial_chunk_cursors() -> [B256; PARALLEL_CHUNKS] {
    let mut cursors = [B256::ZERO; PARALLEL_CHUNKS];
    for (i, c) in cursors.iter_mut().enumerate() {
        *c = chunk_lower(i);
    }
    cursors
}

type DecodedStorageSlots = Vec<(B256, U256)>;

// ──────────────────────────────────────────────────────────────────────────────
// Streaming state download
// ──────────────────────────────────────────────────────────────────────────────

/// Result of a [`download_state`] call.
#[derive(Debug)]
pub enum DownloadStateOutcome {
    /// All chunks completed — download is finished.
    Done,
    /// At least one chunk hit a stale root. The caller should advance the
    /// pivot and call [`download_state`] again with the same `cursors`
    /// (mutated in place to per-chunk resume points).
    Stale,
}

/// Per-chunk outcome.
#[derive(Debug)]
enum ChunkOutcome {
    Done,
    Stale { resume_from: B256 },
}

/// Downloads state (accounts, storage, bytecodes) at `root_hash` by splitting
/// the B256 hash space into [`PARALLEL_CHUNKS`] equal ranges and downloading
/// them concurrently.
///
/// `cursors` holds the resume point for each chunk and is mutated in place:
/// completed chunks are advanced past their upper bound; stale chunks are
/// rewound to the start of the in-flight batch. On a fresh start use
/// [`initial_chunk_cursors`].
pub async fn download_state<C, F>(
    client: &C,
    factory: &F,
    root_hash: B256,
    cursors: &mut [B256; PARALLEL_CHUNKS],
) -> Result<DownloadStateOutcome, SnapSyncError>
where
    C: SnapClient + Clone + Send + Sync + 'static,
    F: DatabaseProviderFactory + Clone + Send + Sync + 'static,
    F::Provider: DBProvider + HeaderProvider,
    F::ProviderRW: DBProvider + StateWriter,
    <F::ProviderRW as DBProvider>::Tx: DbTxMut,
{
    let request_id = Arc::new(AtomicU64::new(0));

    // Spawn the global writer task. All chunks send their accumulated writes
    // here; the writer commits them in batched transactions, eliminating
    // MDBX writer-lock contention between chunks.
    let (writer_tx, writer_handle) = spawn_writer_task(factory.clone());

    // Bound concurrent chunk tasks so we don't overwhelm the serving peer.
    // We still spawn one task per remaining chunk so finished workers can
    // pick up the next pending range (work-stealing).
    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_CHUNKS));

    // Spawn each chunk on its own tokio task so CPU-heavy work (proof
    // verification, RLP decode, keccak) can run on different worker threads.
    let mut tasks: JoinSet<(usize, B256, Result<ChunkOutcome, SnapSyncError>)> = JoinSet::new();
    for i in 0..PARALLEL_CHUNKS {
        let start = cursors[i];
        let limit = chunk_upper(i);
        // Skip chunks that are already complete (e.g. resumed across stale roots).
        if start >= limit {
            continue;
        }
        let client = client.clone();
        let writer_tx = writer_tx.clone();
        let request_id = Arc::clone(&request_id);
        let semaphore = Arc::clone(&semaphore);
        tasks.spawn(async move {
            let _permit = semaphore.acquire().await.expect("semaphore not closed");
            let outcome =
                download_state_chunk(&client, root_hash, start, limit, &request_id, i, &writer_tx)
                    .await;
            (i, limit, outcome)
        });
    }
    // Drop our reference to the sender so the writer exits once all chunk
    // tasks complete (and their sender clones drop).
    drop(writer_tx);

    let mut any_stale = false;
    let mut chunk_error: Option<SnapSyncError> = None;
    while let Some(joined) = tasks.join_next().await {
        let (i, limit, outcome) =
            joined.map_err(|e| SnapSyncError::Network(format!("snap chunk task failed: {e}")))?;
        match outcome {
            Ok(ChunkOutcome::Done) => {
                // Mark chunk fully consumed so subsequent calls skip it.
                cursors[i] = limit;
            }
            Ok(ChunkOutcome::Stale { resume_from }) => {
                cursors[i] = resume_from;
                any_stale = true;
            }
            Err(e) => {
                if chunk_error.is_none() {
                    chunk_error = Some(e);
                }
            }
        }
    }

    // Wait for the writer to drain and commit any remaining buffered batches.
    let writer_result = writer_handle
        .await
        .map_err(|e| SnapSyncError::Database(format!("snap writer task panicked: {e}")))?;

    if let Some(e) = chunk_error {
        return Err(e);
    }
    writer_result?;

    Ok(if any_stale { DownloadStateOutcome::Stale } else { DownloadStateOutcome::Done })
}

/// Downloads accounts, storage, and bytecodes within `[starting_hash, limit_hash)`
/// for a single chunk. Writes are accumulated locally and handed off to the
/// global writer task via `writer`.
async fn download_state_chunk<C>(
    client: &C,
    root_hash: B256,
    starting_hash: B256,
    limit_hash: B256,
    request_id: &AtomicU64,
    chunk_idx: usize,
    writer: &mpsc::Sender<PendingWrites>,
) -> Result<ChunkOutcome, SnapSyncError>
where
    C: SnapClient + 'static,
{
    if starting_hash >= limit_hash {
        return Ok(ChunkOutcome::Done);
    }

    let mut cursor = starting_hash;
    let mut pending = PendingWrites::new();

    loop {
        // Remember the start of this batch so we can resume here if any
        // sub-fetch (accounts, storage, bytecodes) hits a stale root.
        let batch_start = cursor;

        // ── Fetch account batch ──────────────────────────────────────────

        let req_id = request_id.fetch_add(1, Ordering::Relaxed);
        let request = GetAccountRangeMessage {
            request_id: req_id,
            root_hash,
            starting_hash: cursor,
            limit_hash,
            response_bytes: SNAP_RESPONSE_BYTES_LIMIT,
        };

        info!(
            target: "engine::snap_sync",
            request_id = req_id,
            chunk = chunk_idx,
            %root_hash,
            starting_hash = %cursor,
            limit_hash = %limit_hash,
            "Requesting account range"
        );

        let response = client.get_account_range(request).await.map_err(|e| {
            SnapSyncError::Network(format!("snap account range request failed: {e}"))
        })?;
        let msg = match response.into_data() {
            SnapResponse::AccountRange(msg) => msg,
            _ => return Err(SnapSyncError::Network("unexpected snap response type".into())),
        };

        if msg.accounts.is_empty() {
            if msg.proof.is_empty() {
                handoff(&mut pending, writer).await?;
                return Ok(ChunkOutcome::Stale { resume_from: cursor });
            }
            verify_account_range_proof(root_hash, cursor, &[], &msg.proof)?;
            handoff(&mut pending, writer).await?;
            return Ok(ChunkOutcome::Done);
        }

        // ── Decode + write accounts ──────────────────────────────────────

        let mut account_batch = Vec::with_capacity(msg.accounts.len());
        let mut batch_code_hashes = HashSet::new();
        // Representative-only storage_root map: each unique storage_root maps
        // to exactly one (lowest-hash) representative account from the batch.
        // The peer is queried only for representatives; the returned slots are
        // then applied to every account sharing the same storage_root via
        // `dup_accounts` below.
        let mut representative_for_root: HashMap<B256, B256> = HashMap::new();
        let mut batch_storage_roots: HashMap<B256, B256> = HashMap::new();
        let mut batch_account_hashes: Vec<B256> = Vec::new();
        let mut dup_accounts: HashMap<B256, Vec<B256>> = HashMap::new();
        let mut decoded_accounts = Vec::with_capacity(msg.accounts.len());
        let mut previous_hash = None;

        for account_data in &msg.accounts {
            if account_data.hash < cursor ||
                previous_hash.is_some_and(|previous| account_data.hash <= previous)
            {
                return Err(SnapSyncError::Network(
                    "snap account range returned non-monotonic account hashes".into(),
                ));
            }

            let trie_account = account_data.account;
            let account = Account::from(trie_account);

            if let Some(code_hash) = account.bytecode_hash {
                batch_code_hashes.insert(code_hash);
            }

            previous_hash = Some(account_data.hash);
            // Skip accounts with empty storage trie (most EOAs and unused
            // contract slots). Otherwise dedup by storage_root: the same
            // storage trie may back multiple accounts (e.g. EIP-1167 clones,
            // identical proxies); query it once and replay results.
            if trie_account.storage_root != EMPTY_ROOT_HASH {
                let storage_root = trie_account.storage_root;
                match representative_for_root.get(&storage_root).copied() {
                    Some(rep) => {
                        dup_accounts.entry(rep).or_default().push(account_data.hash);
                    }
                    None => {
                        representative_for_root.insert(storage_root, account_data.hash);
                        batch_storage_roots.insert(account_data.hash, storage_root);
                        batch_account_hashes.push(account_data.hash);
                    }
                }
            }
            decoded_accounts.push((account_data.hash, trie_account));
            account_batch.push((account_data.hash, account));
        }

        verify_account_range_proof(root_hash, cursor, &decoded_accounts, &msg.proof)?;

        info!(
            target: "engine::snap_sync",
            request_id = req_id,
            chunk = chunk_idx,
            accounts = account_batch.len(),
            %root_hash,
            starting_hash = %batch_start,
            first_hash = %account_batch.first().expect("non-empty above").0,
            last_hash = %account_batch.last().expect("non-empty above").0,
            "Downloaded account range"
        );
        pending.accounts.extend(account_batch);

        // ── Fetch storage for this batch ─────────────────────────────────
        // If the peer returns empty (stale root), flush what we have and
        // resume the entire batch from `batch_start` with a fresh root.

        if fetch_storage_for_accounts(
            client,
            root_hash,
            &batch_account_hashes,
            &batch_storage_roots,
            &dup_accounts,
            request_id,
            &mut pending,
        )
        .await?
        {
            handoff(&mut pending, writer).await?;
            return Ok(ChunkOutcome::Stale { resume_from: batch_start });
        }

        // ── Fetch bytecodes for this batch ───────────────────────────────

        fetch_bytecodes(client, &batch_code_hashes, request_id, &mut pending).await?;

        // Hand off to the writer task once buffered enough rows.
        maybe_handoff(&mut pending, writer).await?;

        // ── Advance cursor ───────────────────────────────────────────────

        let last_hash = msg.accounts.last().expect("checked non-empty above").hash;
        let next = increment_b256(last_hash);
        // Wraparound (last_hash == MAX_HASH) or reached chunk's upper bound.
        if next == B256::ZERO || next >= limit_hash {
            handoff(&mut pending, writer).await?;
            return Ok(ChunkOutcome::Done);
        }
        cursor = next;
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Storage download (per-batch)
// ──────────────────────────────────────────────────────────────────────────────

/// Fetches and writes storage for a batch of account hashes.
///
/// Returns `Ok(true)` if the serving peer returned empty (stale root),
/// `Ok(false)` if all storage was fetched successfully.
///
/// Handles the snap protocol's response-size truncation: if the last account
/// in a multi-account response has a proof attached, its storage was incomplete
/// and we issue continuation requests for that account before moving on.
async fn fetch_storage_for_accounts<C>(
    client: &C,
    root_hash: B256,
    account_hashes: &[B256],
    storage_roots: &HashMap<B256, B256>,
    dup_accounts: &HashMap<B256, Vec<B256>>,
    request_id: &AtomicU64,
    pending: &mut PendingWrites,
) -> Result<bool, SnapSyncError>
where
    C: SnapClient + 'static,
{
    // Pre-split into fixed STORAGE_BATCH_SIZE batches so requests are
    // independent and dispatchable in parallel. Each batch's truncation
    // (proof on last account) is handled within its own future.
    let batches: Vec<&[B256]> = account_hashes.chunks(STORAGE_BATCH_SIZE).collect();
    let mut next_batch = 0;
    let mut in_flight = FuturesUnordered::new();

    // Prime up to STORAGE_PARALLELISM in-flight requests, then refill as
    // each one completes.
    while next_batch < batches.len() && in_flight.len() < STORAGE_PARALLELISM {
        let chunk = batches[next_batch];
        in_flight.push(fetch_storage_batch(client, root_hash, chunk, storage_roots, request_id));
        next_batch += 1;
    }

    while let Some(result) = in_flight.next().await {
        match result? {
            StorageBatchOutcome::Stale => return Ok(true),
            StorageBatchOutcome::Complete(entries) => {
                // Expand entries to cover accounts that share a storage_root
                // with a representative.
                for (rep_hash, slot_hash, value) in entries {
                    pending.storages.push((rep_hash, slot_hash, value));
                    if let Some(dups) = dup_accounts.get(&rep_hash) {
                        for dup in dups {
                            pending.storages.push((*dup, slot_hash, value));
                        }
                    }
                }
            }
        }

        if next_batch < batches.len() {
            let chunk = batches[next_batch];
            in_flight.push(fetch_storage_batch(
                client,
                root_hash,
                chunk,
                storage_roots,
                request_id,
            ));
            next_batch += 1;
        }
    }

    Ok(false)
}

/// Outcome of fetching storage for a single STORAGE_BATCH_SIZE-account batch.
enum StorageBatchOutcome {
    /// All accounts verified; entries ready to commit.
    Complete(Vec<(B256, B256, U256)>),
    /// Peer no longer serves this root.
    Stale,
}

/// Fetches and verifies storage for a single batch of up to [`STORAGE_BATCH_SIZE`]
/// accounts. Handles truncation of the last account by issuing continuation
/// requests until that account's storage is complete.
async fn fetch_storage_batch<C>(
    client: &C,
    root_hash: B256,
    chunk: &[B256],
    storage_roots: &HashMap<B256, B256>,
    request_id: &AtomicU64,
) -> Result<StorageBatchOutcome, SnapSyncError>
where
    C: SnapClient + 'static,
{
    let mut entries: Vec<(B256, B256, U256)> = Vec::new();
    let mut idx = 0;

    while idx < chunk.len() {
        let remaining = &chunk[idx..];
        let req_id = request_id.fetch_add(1, Ordering::Relaxed);
        let request = GetStorageRangesMessage {
            request_id: req_id,
            root_hash,
            account_hashes: remaining.to_vec(),
            starting_hash: B256::ZERO,
            limit_hash: MAX_HASH,
            response_bytes: SNAP_RESPONSE_BYTES_LIMIT,
        };

        let response = client.get_storage_ranges(request).await.map_err(|e| {
            SnapSyncError::Network(format!("snap storage range request failed: {e}"))
        })?;
        let msg = match response.into_data() {
            SnapResponse::StorageRanges(msg) => msg,
            _ => return Err(SnapSyncError::Network("unexpected snap response type".into())),
        };

        if msg.slots.len() > remaining.len() {
            return Err(SnapSyncError::Network(
                "snap storage range returned more slot lists than requested".into(),
            ));
        }

        let returned_count = msg.slots.len();

        // Empty outer vec → peer no longer has the requested root (stale).
        if returned_count == 0 {
            return Ok(StorageBatchOutcome::Stale);
        }

        let has_proof = !msg.proof.is_empty();
        let proof_index = has_proof.then_some(returned_count - 1);
        for (i, slots) in msg.slots.iter().enumerate() {
            let account_hash = remaining[i];
            validate_storage_slots(account_hash, B256::ZERO, MAX_HASH, slots)?;

            let account_slots = if Some(i) == proof_index {
                verify_storage_range_proof(
                    account_hash,
                    storage_roots,
                    B256::ZERO,
                    slots,
                    &msg.proof,
                )?;

                if slots.is_empty() {
                    verify_full_storage_range(account_hash, storage_roots, slots)?
                } else {
                    let resume_from =
                        increment_b256(slots.last().expect("slots is not empty").hash);
                    match fetch_storage_continuation(
                        client,
                        root_hash,
                        account_hash,
                        storage_roots,
                        resume_from,
                        request_id,
                        slots.clone(),
                    )
                    .await?
                    {
                        StorageContinuationOutcome::Complete(slots) => slots,
                        StorageContinuationOutcome::Stale => return Ok(StorageBatchOutcome::Stale),
                    }
                }
            } else {
                verify_full_storage_range(account_hash, storage_roots, slots)?
            };

            entries.extend(
                account_slots
                    .into_iter()
                    .map(|(slot_hash, value)| (account_hash, slot_hash, value)),
            );
        }

        // If the last account was truncated (proof attached) we already fully
        // resolved it via continuation, so we advance past it. Otherwise we
        // advance by `returned_count`, which may be less than `remaining` if
        // the response itself was truncated mid-batch.
        idx += returned_count;
    }

    Ok(StorageBatchOutcome::Complete(entries))
}

/// Continuation result for a single-account storage range.
enum StorageContinuationOutcome {
    /// The account storage is complete and verified against its root.
    Complete(DecodedStorageSlots),
    /// The serving peer no longer has the requested root or account.
    Stale,
}

/// Continuation loop for a single account whose storage was truncated.
async fn fetch_storage_continuation<C>(
    client: &C,
    root_hash: B256,
    account_hash: B256,
    storage_roots: &HashMap<B256, B256>,
    mut starting_hash: B256,
    request_id: &AtomicU64,
    mut collected_slots: Vec<StorageData>,
) -> Result<StorageContinuationOutcome, SnapSyncError>
where
    C: SnapClient + 'static,
{
    loop {
        let req_id = request_id.fetch_add(1, Ordering::Relaxed);
        let request = GetStorageRangesMessage {
            request_id: req_id,
            root_hash,
            account_hashes: vec![account_hash],
            starting_hash,
            limit_hash: MAX_HASH,
            response_bytes: SNAP_RESPONSE_BYTES_LIMIT,
        };

        let response = client.get_storage_ranges(request).await.map_err(|e| {
            SnapSyncError::Network(format!("snap storage continuation failed: {e}"))
        })?;
        let msg = match response.into_data() {
            SnapResponse::StorageRanges(msg) => msg,
            _ => return Err(SnapSyncError::Network("unexpected snap response type".into())),
        };

        if msg.slots.len() > 1 {
            return Err(SnapSyncError::Network(
                "snap storage continuation returned multiple slot lists".into(),
            ));
        }

        let Some(slots) = msg.slots.first() else {
            return Ok(StorageContinuationOutcome::Stale);
        };

        if slots.is_empty() {
            if !msg.proof.is_empty() {
                verify_storage_range_proof(
                    account_hash,
                    storage_roots,
                    starting_hash,
                    slots,
                    &msg.proof,
                )?;
            }
            let decoded = verify_full_storage_range(account_hash, storage_roots, &collected_slots)?;
            return Ok(StorageContinuationOutcome::Complete(decoded));
        }

        validate_storage_slots(account_hash, starting_hash, MAX_HASH, slots)?;

        if !msg.proof.is_empty() {
            verify_storage_range_proof(
                account_hash,
                storage_roots,
                starting_hash,
                slots,
                &msg.proof,
            )?;
        }

        collected_slots.extend_from_slice(slots);
        if msg.proof.is_empty() {
            let decoded = verify_full_storage_range(account_hash, storage_roots, &collected_slots)?;
            return Ok(StorageContinuationOutcome::Complete(decoded));
        }

        starting_hash = increment_b256(slots.last().unwrap().hash);
    }
}

fn validate_storage_slots(
    account_hash: B256,
    starting_hash: B256,
    limit_hash: B256,
    slots: &[StorageData],
) -> Result<(), SnapSyncError> {
    let mut previous = None;
    for slot in slots {
        if slot.hash < starting_hash || slot.hash >= limit_hash {
            return Err(SnapSyncError::Network(format!(
                "snap storage range for account {account_hash} returned slot outside requested bounds"
            )))
        }
        if previous.is_some_and(|previous| slot.hash <= previous) {
            return Err(SnapSyncError::Network(format!(
                "snap storage range for account {account_hash} returned non-monotonic slots"
            )))
        }
        previous = Some(slot.hash);
    }
    Ok(())
}

fn verify_full_storage_range(
    account_hash: B256,
    storage_roots: &HashMap<B256, B256>,
    slots: &[StorageData],
) -> Result<DecodedStorageSlots, SnapSyncError> {
    let Some(expected_root) = storage_roots.get(&account_hash).copied() else {
        return Err(SnapSyncError::Network(format!(
            "snap storage response for unknown account {account_hash}"
        )));
    };

    let decoded = decode_storage_slots(slots)?;

    let got = storage_root(decoded.iter().copied());
    if got != expected_root {
        return Err(SnapSyncError::Network(format!(
            "snap full storage range root mismatch for account {account_hash}: expected {expected_root}, got {got}"
        )))
    }

    Ok(decoded)
}

fn verify_account_range_proof(
    root_hash: B256,
    starting_hash: B256,
    accounts: &[(B256, TrieAccount)],
    proof: &[Bytes],
) -> Result<(), SnapSyncError> {
    let leaves =
        accounts.iter().copied().map(|(hash, account)| (hash, account_trie_value(account)));

    verify_range_proof(root_hash, starting_hash, leaves, proof)
        .map_err(|e| SnapSyncError::Network(format!("invalid snap account range proof: {e}")))
}

fn verify_storage_range_proof(
    account_hash: B256,
    storage_roots: &HashMap<B256, B256>,
    starting_hash: B256,
    slots: &[StorageData],
    proof: &[Bytes],
) -> Result<DecodedStorageSlots, SnapSyncError> {
    let Some(storage_root) = storage_roots.get(&account_hash).copied() else {
        return Err(SnapSyncError::Network(format!(
            "snap storage proof for unknown account {account_hash}"
        )));
    };

    let decoded = decode_storage_slots(slots)?;
    let leaves = decoded
        .iter()
        .map(|(hash, value)| (*hash, alloy_rlp::encode_fixed_size(value).as_ref().to_vec()));

    verify_range_proof(storage_root, starting_hash, leaves, proof)
        .map_err(|e| SnapSyncError::Network(format!("invalid snap storage range proof: {e}")))?;

    Ok(decoded)
}

fn account_trie_value(account: TrieAccount) -> Vec<u8> {
    alloy_rlp::encode(account)
}

fn decode_storage_slots(slots: &[StorageData]) -> Result<DecodedStorageSlots, SnapSyncError> {
    slots
        .iter()
        .map(|slot| {
            let value = slot
                .decode_value()
                .map_err(|e| SnapSyncError::RlpDecode(format!("snap storage decode: {e}")))?;
            Ok((slot.hash, value))
        })
        .collect()
}

// ──────────────────────────────────────────────────────────────────────────────
// Bytecode download (per-batch)
// ──────────────────────────────────────────────────────────────────────────────

/// Fetches and writes bytecodes for a set of code hashes.
async fn fetch_bytecodes<C>(
    client: &C,
    code_hashes: &HashSet<B256>,
    request_id: &AtomicU64,
    pending: &mut PendingWrites,
) -> Result<(), SnapSyncError>
where
    C: SnapClient + 'static,
{
    let hashes: Vec<B256> = code_hashes.iter().copied().collect();
    for chunk in hashes.chunks(BYTECODE_BATCH_SIZE) {
        let req_id = request_id.fetch_add(1, Ordering::Relaxed);
        let request = GetByteCodesMessage {
            request_id: req_id,
            hashes: chunk.to_vec(),
            response_bytes: SNAP_RESPONSE_BYTES_LIMIT,
        };

        let response = client
            .get_byte_codes(request)
            .await
            .map_err(|e| SnapSyncError::Network(format!("snap bytecode request failed: {e}")))?;
        let msg = match response.into_data() {
            SnapResponse::ByteCodes(msg) => msg,
            _ => return Err(SnapSyncError::Network("unexpected snap response type".into())),
        };

        let codes = match_bytecodes_to_hashes(chunk, &msg.codes)?;
        pending.bytecodes.extend(codes);
    }

    Ok(())
}

fn match_bytecodes_to_hashes(
    requested_hashes: &[B256],
    codes: &[Bytes],
) -> Result<Vec<(B256, Bytes)>, SnapSyncError> {
    let requested: HashMap<_, _> =
        requested_hashes.iter().copied().enumerate().map(|(i, hash)| (hash, i)).collect();
    let mut seen = HashSet::new();
    let mut last_position = None;
    let mut matched = Vec::with_capacity(codes.len());

    for code in codes {
        let hash = keccak256(code.as_ref());
        let Some(position) = requested.get(&hash).copied() else {
            return Err(SnapSyncError::Network(format!(
                "snap bytecode response contained unrequested code hash {hash}"
            )))
        };
        if last_position.is_some_and(|last| position <= last) {
            return Err(SnapSyncError::Network(
                "snap bytecode response was not in request order".into(),
            ));
        }
        if !seen.insert(hash) {
            return Err(SnapSyncError::Network(format!(
                "snap bytecode response duplicated code hash {hash}"
            )))
        }
        last_position = Some(position);
        matched.push((hash, code.clone()));
    }

    Ok(matched)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b256_from_u64(value: u64) -> B256 {
        B256::left_padding_from(&value.to_be_bytes())
    }

    #[test]
    fn bytecode_matching_uses_returned_code_hashes() {
        let first = Bytes::from_static(&[1, 2, 3]);
        let second = Bytes::from_static(&[4, 5, 6]);
        let requested = vec![keccak256(second.as_ref()), keccak256(first.as_ref())];

        let matched = match_bytecodes_to_hashes(&requested, &[first.clone()]).unwrap();

        assert_eq!(matched, vec![(keccak256(first.as_ref()), first)]);
    }

    #[test]
    fn bytecode_matching_rejects_unrequested_code() {
        let requested = vec![keccak256([1, 2, 3])];
        let unrequested = Bytes::from_static(&[4, 5, 6]);

        assert!(match_bytecodes_to_hashes(&requested, &[unrequested]).is_err());
    }

    #[test]
    fn bytecode_matching_rejects_out_of_order_codes() {
        let first = Bytes::from_static(&[1, 2, 3]);
        let second = Bytes::from_static(&[4, 5, 6]);
        let requested = vec![keccak256(first.as_ref()), keccak256(second.as_ref())];

        assert!(match_bytecodes_to_hashes(&requested, &[second, first]).is_err());
    }

    #[test]
    fn storage_slots_must_be_ordered_within_bounds() {
        let account = b256_from_u64(1);
        let first = StorageData::from_value(b256_from_u64(2), alloy_primitives::U256::from(2));
        let second = StorageData::from_value(b256_from_u64(3), alloy_primitives::U256::from(3));

        assert!(validate_storage_slots(
            account,
            b256_from_u64(2),
            b256_from_u64(4),
            &[first.clone(), second.clone()]
        )
        .is_ok());
        assert!(validate_storage_slots(
            account,
            b256_from_u64(2),
            b256_from_u64(4),
            &[second, first]
        )
        .is_err());
    }

    #[test]
    fn full_storage_range_verifies_storage_root() {
        let account = b256_from_u64(1);
        let slot = b256_from_u64(2);
        let value = alloy_primitives::U256::from(3);
        let storage_roots = HashMap::from([(account, storage_root([(slot, value)]))]);
        let slots = vec![StorageData::from_value(slot, value)];

        assert!(verify_full_storage_range(account, &storage_roots, &slots).is_ok());
        assert!(verify_full_storage_range(account, &storage_roots, &[]).is_err());
    }
}

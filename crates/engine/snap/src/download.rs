//! Snap sync download loops for accounts, storage, and bytecodes.
//!
//! The main entry point is [`download_state`] which streams through the entire
//! state trie in account-hash order. For each batch of accounts it immediately
//! fetches the associated storage slots and bytecodes before moving on to the
//! next range. This keeps memory usage bounded to a single batch at a time
//! regardless of total state size.

use crate::{
    storage::{increment_b256, write_bytecodes, write_hashed_accounts, write_hashed_storages},
    SnapSyncError, SNAP_RESPONSE_BYTES_LIMIT,
};
use alloy_primitives::B256;
use reth_db_api::transaction::DbTxMut;
use reth_eth_wire_types::snap::{
    GetAccountRangeMessage, GetByteCodesMessage, GetStorageRangesMessage, SlimAccount,
};
use reth_network_p2p::snap::client::{SnapClient, SnapResponse};
use reth_primitives_traits::Account;
use reth_provider::{DatabaseProviderFactory, HeaderProvider};
use reth_storage_api::{DBProvider, StateWriter};
use std::collections::HashSet;
use tracing::info;

/// Maximum number of account hashes per storage range request.
const STORAGE_BATCH_SIZE: usize = 20;

/// Maximum number of code hashes per bytecode request.
const BYTECODE_BATCH_SIZE: usize = 50;

/// Maximum hash value used as the range upper bound.
const MAX_HASH: B256 = B256::new([0xff; 32]);

// ──────────────────────────────────────────────────────────────────────────────
// Streaming state download
// ──────────────────────────────────────────────────────────────────────────────

/// Result of a [`download_state`] call.
#[derive(Debug)]
pub enum DownloadStateOutcome {
    /// Entire account range was iterated — download is complete.
    Done,
    /// The serving peer returned empty for the requested root (stale).
    /// Contains the `starting_hash` to resume from after the caller
    /// advances the pivot and obtains a fresh root.
    Stale {
        /// The hash to resume downloading from with a fresh root.
        resume_from: B256,
    },
}

/// Downloads state (accounts, storage, bytecodes) at `root_hash`, streaming
/// from `starting_hash` onward.
///
/// For each batch of accounts returned by `GetAccountRange`, the associated
/// storage and bytecodes are fetched and written to MDBX immediately before
/// requesting the next account range. Memory usage is bounded to one batch.
///
/// Returns [`DownloadStateOutcome::Stale`] when the serving peer returns empty
/// (root not available). The caller should advance the pivot to get a new root
/// and call this again with the returned `resume_from` hash — no progress is
/// lost.
pub async fn download_state<C, F>(
    client: &C,
    factory: &F,
    root_hash: B256,
    starting_hash: B256,
) -> Result<DownloadStateOutcome, SnapSyncError>
where
    C: SnapClient + 'static,
    F: DatabaseProviderFactory + Clone + Send + Sync + 'static,
    F::Provider: DBProvider + HeaderProvider,
    F::ProviderRW: DBProvider + StateWriter,
    <F::ProviderRW as DBProvider>::Tx: DbTxMut,
{
    let mut request_id: u64 = 0;
    let mut cursor = starting_hash;

    loop {
        // Remember the start of this batch so we can resume here if any
        // sub-fetch (accounts, storage, bytecodes) hits a stale root.
        let batch_start = cursor;

        // ── Fetch account batch ──────────────────────────────────────────

        request_id += 1;
        let request = GetAccountRangeMessage {
            request_id,
            root_hash,
            starting_hash: cursor,
            limit_hash: MAX_HASH,
            response_bytes: SNAP_RESPONSE_BYTES_LIMIT,
        };

        let response = client.get_account_range(request).await.map_err(|e| {
            SnapSyncError::Network(format!("snap account range request failed: {e}"))
        })?;
        let msg = match response.into_data() {
            SnapResponse::AccountRange(msg) => msg,
            _ => return Err(SnapSyncError::Network("unexpected snap response type".into())),
        };

        if msg.accounts.is_empty() {
            if cursor == starting_hash {
                // Very first request for this root returned empty → stale.
                return Ok(DownloadStateOutcome::Stale { resume_from: cursor });
            }
            return Ok(DownloadStateOutcome::Done);
        }

        // ── Decode + write accounts ──────────────────────────────────────

        let mut account_batch = Vec::with_capacity(msg.accounts.len());
        let mut batch_account_hashes = Vec::with_capacity(msg.accounts.len());
        let mut batch_code_hashes = HashSet::new();

        for account_data in &msg.accounts {
            let account = alloy_rlp::decode_exact::<SlimAccount>(&account_data.body)
                .map(Account::from)
                .map_err(|e| SnapSyncError::RlpDecode(format!("snap account decode: {e}")))?;

            if let Some(code_hash) = account.bytecode_hash {
                batch_code_hashes.insert(code_hash);
            }

            batch_account_hashes.push(account_data.hash);
            account_batch.push((account_data.hash, account));
        }

        info!(
            target: "engine::snap_sync",
            accounts = account_batch.len(),
            %root_hash,
            "Downloaded account range"
        );
        write_hashed_accounts(factory, &account_batch)?;

        // ── Fetch + write storage for this batch ─────────────────────────
        // If the peer returns empty (stale root), return Stale at batch_start
        // so the caller retries the entire batch with a fresh root.

        if fetch_storage_for_accounts(
            client,
            factory,
            root_hash,
            &batch_account_hashes,
            &mut request_id,
        )
        .await?
        {
            return Ok(DownloadStateOutcome::Stale { resume_from: batch_start });
        }

        // ── Fetch + write bytecodes for this batch ───────────────────────

        if fetch_bytecodes(client, factory, &batch_code_hashes, &mut request_id).await? {
            return Ok(DownloadStateOutcome::Stale { resume_from: batch_start });
        }

        // ── Advance cursor ───────────────────────────────────────────────

        let last_hash = msg.accounts.last().expect("checked non-empty above").hash;
        if last_hash == MAX_HASH {
            return Ok(DownloadStateOutcome::Done);
        }
        cursor = increment_b256(last_hash);
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
async fn fetch_storage_for_accounts<C, F>(
    client: &C,
    factory: &F,
    root_hash: B256,
    account_hashes: &[B256],
    request_id: &mut u64,
) -> Result<bool, SnapSyncError>
where
    C: SnapClient + 'static,
    F: DatabaseProviderFactory + Clone + Send + Sync + 'static,
    F::ProviderRW: DBProvider + StateWriter,
    <F::ProviderRW as DBProvider>::Tx: DbTxMut,
{
    let mut idx = 0;

    while idx < account_hashes.len() {
        let end = (idx + STORAGE_BATCH_SIZE).min(account_hashes.len());
        let chunk = &account_hashes[idx..end];

        *request_id += 1;
        let request = GetStorageRangesMessage {
            request_id: *request_id,
            root_hash,
            account_hashes: chunk.to_vec(),
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

        let returned_count = msg.slots.len().min(chunk.len());

        // Empty response for the very first sub-chunk → stale root.
        if returned_count == 0 && idx == 0 {
            return Ok(true);
        }

        // Write all returned slots.
        let mut entries = Vec::new();
        for (i, slots) in msg.slots.iter().enumerate() {
            if i >= chunk.len() {
                break;
            }
            let account_hash = chunk[i];
            for slot in slots {
                let value = slot
                    .decode_value()
                    .map_err(|e| SnapSyncError::RlpDecode(format!("snap storage decode: {e}")))?;
                entries.push((account_hash, slot.hash, value));
            }
        }
        if !entries.is_empty() {
            write_hashed_storages(factory, &entries)?;
        }

        // If a proof is attached, the last account was truncated by the
        // response size limit — issue continuation requests for it.
        let has_proof = !msg.proof.is_empty();
        if has_proof && returned_count > 0 {
            let incomplete_account = chunk[returned_count - 1];
            let last_slots = &msg.slots[returned_count - 1];
            if let Some(last_slot) = last_slots.last() {
                let resume_from = increment_b256(last_slot.hash);
                fetch_storage_continuation(
                    client,
                    factory,
                    root_hash,
                    incomplete_account,
                    resume_from,
                    request_id,
                )
                .await?;
            }
            idx += returned_count;
        } else {
            idx = end;
        }
    }

    Ok(false)
}

/// Continuation loop for a single account whose storage was truncated.
async fn fetch_storage_continuation<C, F>(
    client: &C,
    factory: &F,
    root_hash: B256,
    account_hash: B256,
    mut starting_hash: B256,
    request_id: &mut u64,
) -> Result<(), SnapSyncError>
where
    C: SnapClient + 'static,
    F: DatabaseProviderFactory + Clone + Send + Sync + 'static,
    F::ProviderRW: DBProvider + StateWriter,
    <F::ProviderRW as DBProvider>::Tx: DbTxMut,
{
    loop {
        *request_id += 1;
        let request = GetStorageRangesMessage {
            request_id: *request_id,
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

        let slots = msg.slots.first().cloned().unwrap_or_default();
        if slots.is_empty() {
            break;
        }

        let mut entries = Vec::new();
        for slot in &slots {
            let value = slot
                .decode_value()
                .map_err(|e| SnapSyncError::RlpDecode(format!("snap storage decode: {e}")))?;
            entries.push((account_hash, slot.hash, value));
        }
        write_hashed_storages(factory, &entries)?;

        if msg.proof.is_empty() {
            break;
        }

        starting_hash = increment_b256(slots.last().unwrap().hash);
    }

    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────────
// Bytecode download (per-batch)
// ──────────────────────────────────────────────────────────────────────────────

/// Fetches and writes bytecodes for a set of code hashes.
///
/// Returns `Ok(true)` if the serving peer returned empty (stale root),
/// `Ok(false)` if all bytecodes were fetched successfully.
async fn fetch_bytecodes<C, F>(
    client: &C,
    factory: &F,
    code_hashes: &HashSet<B256>,
    request_id: &mut u64,
) -> Result<bool, SnapSyncError>
where
    C: SnapClient + 'static,
    F: DatabaseProviderFactory + Clone + Send + Sync + 'static,
    F::ProviderRW: DBProvider + StateWriter,
    <F::ProviderRW as DBProvider>::Tx: DbTxMut,
{
    let hashes: Vec<B256> = code_hashes.iter().copied().collect();
    for (chunk_idx, chunk) in hashes.chunks(BYTECODE_BATCH_SIZE).enumerate() {
        *request_id += 1;
        let request = GetByteCodesMessage {
            request_id: *request_id,
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

        if msg.codes.is_empty() && chunk_idx == 0 && !hashes.is_empty() {
            return Ok(true);
        }

        let mut codes = Vec::with_capacity(msg.codes.len());
        for (i, code) in msg.codes.iter().enumerate() {
            if i < chunk.len() {
                codes.push((chunk[i], code.clone()));
            }
        }

        if !codes.is_empty() {
            write_bytecodes(factory, &codes)?;
        }
    }

    Ok(false)
}

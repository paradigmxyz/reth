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
use alloy_primitives::{Bytes, B256};
use reth_db_api::transaction::DbTxMut;
use reth_eth_wire_types::snap::{
    GetAccountRangeMessage, GetByteCodesMessage, GetStorageRangesMessage, TrieAccount,
};
use reth_network_p2p::snap::client::{SnapClient, SnapResponse};
use reth_primitives_traits::Account;
use reth_provider::{DatabaseProviderFactory, HeaderProvider};
use reth_storage_api::{DBProvider, StateWriter};
use reth_trie::{verify_unordered_proof, Nibbles};
use std::collections::{HashMap, HashSet};
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
        let mut batch_storage_roots = HashMap::with_capacity(msg.accounts.len());
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
            batch_storage_roots.insert(account_data.hash, trie_account.storage_root);
            decoded_accounts.push((account_data.hash, trie_account));
            batch_account_hashes.push(account_data.hash);
            account_batch.push((account_data.hash, account));
        }

        verify_account_range_proof(root_hash, cursor, &decoded_accounts, &msg.proof)?;

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
            &batch_storage_roots,
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
    storage_roots: &HashMap<B256, B256>,
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
        if returned_count == 0 {
            return Err(SnapSyncError::Network("snap storage range returned no progress".into()));
        }

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

        if !msg.proof.is_empty() && returned_count > 0 {
            let proof_account = chunk[returned_count - 1];
            let proof_slots = &msg.slots[returned_count - 1];
            verify_storage_range_proof(
                proof_account,
                storage_roots,
                B256::ZERO,
                proof_slots,
                &msg.proof,
            )?;
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
                    storage_roots,
                    resume_from,
                    request_id,
                )
                .await?;
            }
            idx += returned_count;
        } else if returned_count < chunk.len() {
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
    storage_roots: &HashMap<B256, B256>,
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

        if !msg.proof.is_empty() {
            verify_storage_range_proof(
                account_hash,
                storage_roots,
                starting_hash,
                &slots,
                &msg.proof,
            )?;
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

fn verify_account_range_proof(
    root_hash: B256,
    starting_hash: B256,
    accounts: &[(B256, TrieAccount)],
    proof: &[Bytes],
) -> Result<(), SnapSyncError> {
    let Some((first_hash, first_account)) = accounts.first().copied() else {
        return Ok(());
    };

    let expected_start = (first_hash == starting_hash).then(|| account_trie_value(first_account));
    verify_trie_value(root_hash, starting_hash, expected_start, proof, "account range start")?;

    let (last_hash, last_account) =
        accounts.last().copied().expect("first account exists, so last account exists");
    verify_trie_value(
        root_hash,
        last_hash,
        Some(account_trie_value(last_account)),
        proof,
        "account range end",
    )
}

fn verify_storage_range_proof(
    account_hash: B256,
    storage_roots: &HashMap<B256, B256>,
    starting_hash: B256,
    slots: &[reth_eth_wire_types::snap::StorageData],
    proof: &[Bytes],
) -> Result<(), SnapSyncError> {
    let Some(storage_root) = storage_roots.get(&account_hash).copied() else {
        return Err(SnapSyncError::Network(format!(
            "snap storage proof for unknown account {account_hash}"
        )));
    };

    let Some(first_slot) = slots.first() else {
        return verify_trie_value(storage_root, starting_hash, None, proof, "empty storage range");
    };

    let expected_start = if first_slot.hash == starting_hash {
        let value = first_slot
            .decode_value()
            .map_err(|e| SnapSyncError::RlpDecode(format!("snap storage decode: {e}")))?;
        (!value.is_zero()).then(|| alloy_rlp::encode_fixed_size(&value).as_ref().to_vec())
    } else {
        None
    };
    verify_trie_value(storage_root, starting_hash, expected_start, proof, "storage range start")?;

    let last_slot = slots.last().expect("first slot exists, so last slot exists");
    let value = last_slot
        .decode_value()
        .map_err(|e| SnapSyncError::RlpDecode(format!("snap storage decode: {e}")))?;

    let expected =
        (!value.is_zero()).then(|| alloy_rlp::encode_fixed_size(&value).as_ref().to_vec());
    verify_trie_value(storage_root, last_slot.hash, expected, proof, "storage range end")
}

fn verify_trie_value(
    root: B256,
    key: B256,
    expected: Option<Vec<u8>>,
    proof: &[Bytes],
    context: &'static str,
) -> Result<(), SnapSyncError> {
    let nibbles = Nibbles::unpack(key);
    verify_unordered_proof(root, nibbles, expected, proof)
        .map_err(|e| SnapSyncError::Network(format!("invalid snap {context} proof: {e}")))
}

fn account_trie_value(account: TrieAccount) -> Vec<u8> {
    alloy_rlp::encode(account)
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

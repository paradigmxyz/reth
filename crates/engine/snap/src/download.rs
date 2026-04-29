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
use alloy_primitives::{keccak256, Bytes, B256, U256};
use reth_db_api::transaction::DbTxMut;
use reth_eth_wire_types::snap::{
    GetAccountRangeMessage, GetByteCodesMessage, GetStorageRangesMessage, StorageData, TrieAccount,
};
use reth_network_p2p::snap::client::{SnapClient, SnapResponse};
use reth_primitives_traits::Account;
use reth_provider::{DatabaseProviderFactory, HeaderProvider};
use reth_storage_api::{DBProvider, StateWriter};
use reth_trie::root::storage_root;
use std::collections::{HashMap, HashSet};
use tracing::info;

/// Maximum number of account hashes per storage range request.
const STORAGE_BATCH_SIZE: usize = 20;

/// Maximum number of code hashes per bytecode request.
const BYTECODE_BATCH_SIZE: usize = 50;

/// Maximum hash value used as the range upper bound.
const MAX_HASH: B256 = B256::new([0xff; 32]);

type DecodedStorageSlots = Vec<(B256, U256)>;

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
            if msg.proof.is_empty() {
                return Ok(DownloadStateOutcome::Stale { resume_from: cursor });
            }
            verify_account_range_proof(root_hash, cursor, &[], &msg.proof)?;
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

        fetch_bytecodes(client, factory, &batch_code_hashes, &mut request_id).await?;

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

        if msg.slots.len() > chunk.len() {
            return Err(SnapSyncError::Network(
                "snap storage range returned more slot lists than requested".into(),
            ));
        }

        let returned_count = msg.slots.len();

        // Empty response for the very first sub-chunk → stale root.
        if returned_count == 0 && idx == 0 {
            return Ok(true);
        }
        if returned_count == 0 {
            return Err(SnapSyncError::Network("snap storage range returned no progress".into()));
        }

        let has_proof = !msg.proof.is_empty();
        let proof_index = has_proof.then_some(returned_count - 1);
        let mut entries = Vec::new();
        for (i, slots) in msg.slots.iter().enumerate() {
            let account_hash = chunk[i];
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
                        StorageContinuationOutcome::Stale => return Ok(true),
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

        if !entries.is_empty() {
            write_hashed_storages(factory, &entries)?;
        }

        if has_proof {
            idx += returned_count;
        } else if returned_count < chunk.len() {
            idx += returned_count;
        } else {
            idx = end;
        }
    }

    Ok(false)
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
    request_id: &mut u64,
    mut collected_slots: Vec<StorageData>,
) -> Result<StorageContinuationOutcome, SnapSyncError>
where
    C: SnapClient + 'static,
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
async fn fetch_bytecodes<C, F>(
    client: &C,
    factory: &F,
    code_hashes: &HashSet<B256>,
    request_id: &mut u64,
) -> Result<(), SnapSyncError>
where
    C: SnapClient + 'static,
    F: DatabaseProviderFactory + Clone + Send + Sync + 'static,
    F::ProviderRW: DBProvider + StateWriter,
    <F::ProviderRW as DBProvider>::Tx: DbTxMut,
{
    let hashes: Vec<B256> = code_hashes.iter().copied().collect();
    for chunk in hashes.chunks(BYTECODE_BATCH_SIZE) {
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

        let codes = match_bytecodes_to_hashes(chunk, &msg.codes)?;

        if !codes.is_empty() {
            write_bytecodes(factory, &codes)?;
        }
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

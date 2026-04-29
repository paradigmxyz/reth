//! Snap sync download loops for accounts, storage, and bytecodes.
//!
//! The main entry point is [`download_state`] which streams through the entire
//! state trie in account-hash order. For each batch of accounts it immediately
//! fetches the associated storage slots and bytecodes before moving on to the
//! next range. This keeps memory usage bounded to a single batch at a time
//! regardless of total state size.

use crate::SnapSyncError;
use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_primitives::{Bytes, B256, U256};
use alloy_rlp::Decodable;
use reth_db_api::{
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_eth_wire_types::snap::{
    GetAccountRangeMessage, GetByteCodesMessage, GetStorageRangesMessage,
};
use reth_network_p2p::snap::client::{SnapClient, SnapResponse};
use reth_primitives_traits::{Account, Bytecode, StorageEntry};
use reth_provider::{DatabaseProviderFactory, HeaderProvider};
use reth_storage_api::DBProvider;
use std::collections::HashSet;
use tracing::info;

/// Soft response size limit per snap request (2 MiB).
const RESPONSE_BYTES_LIMIT: u64 = 2 * 1024 * 1024;

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
    F::ProviderRW: DBProvider,
    <F::Provider as DBProvider>::Tx: DbTx,
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
            response_bytes: RESPONSE_BYTES_LIMIT,
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
            let account = decode_slim_account(&account_data.body)?;

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
    F::ProviderRW: DBProvider,
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
            response_bytes: RESPONSE_BYTES_LIMIT,
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
                let value = decode_storage_value(&slot.data)?;
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
    F::ProviderRW: DBProvider,
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
            response_bytes: RESPONSE_BYTES_LIMIT,
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
            let value = decode_storage_value(&slot.data)?;
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
    F::ProviderRW: DBProvider,
    <F::ProviderRW as DBProvider>::Tx: DbTxMut,
{
    let hashes: Vec<B256> = code_hashes.iter().copied().collect();
    for (chunk_idx, chunk) in hashes.chunks(BYTECODE_BATCH_SIZE).enumerate() {
        *request_id += 1;
        let request = GetByteCodesMessage {
            request_id: *request_id,
            hashes: chunk.to_vec(),
            response_bytes: RESPONSE_BYTES_LIMIT,
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

// ──────────────────────────────────────────────────────────────────────────────
// MDBX write helpers
// ──────────────────────────────────────────────────────────────────────────────

/// Clears all hashed state tables.
pub(crate) fn clear_hashed_state<F>(factory: &F) -> Result<(), SnapSyncError>
where
    F: DatabaseProviderFactory,
    F::ProviderRW: DBProvider,
    <F::ProviderRW as DBProvider>::Tx: DbTxMut,
{
    let provider = factory.database_provider_rw().map_err(db_err)?;
    {
        let tx = provider.tx_ref();
        tx.clear::<tables::HashedAccounts>().map_err(db_err)?;
        tx.clear::<tables::HashedStorages>().map_err(db_err)?;
        tx.clear::<tables::AccountsTrie>().map_err(db_err)?;
        tx.clear::<tables::StoragesTrie>().map_err(db_err)?;
    }
    provider.commit().map_err(db_err)?;
    Ok(())
}

/// Writes a batch of hashed accounts.
pub(crate) fn write_hashed_accounts<F>(
    factory: &F,
    accounts: &[(B256, Account)],
) -> Result<(), SnapSyncError>
where
    F: DatabaseProviderFactory,
    F::ProviderRW: DBProvider,
    <F::ProviderRW as DBProvider>::Tx: DbTxMut,
{
    let provider = factory.database_provider_rw().map_err(db_err)?;
    {
        let tx = provider.tx_ref();
        for (hash, account) in accounts {
            tx.put::<tables::HashedAccounts>(*hash, *account).map_err(db_err)?;
        }
    }
    provider.commit().map_err(db_err)?;
    Ok(())
}

/// Writes a batch of hashed storage entries.
pub(crate) fn write_hashed_storages<F>(
    factory: &F,
    entries: &[(B256, B256, U256)],
) -> Result<(), SnapSyncError>
where
    F: DatabaseProviderFactory,
    F::ProviderRW: DBProvider,
    <F::ProviderRW as DBProvider>::Tx: DbTxMut,
{
    let provider = factory.database_provider_rw().map_err(db_err)?;
    {
        let tx = provider.tx_ref();
        for &(account_hash, slot_hash, value) in entries {
            tx.put::<tables::HashedStorages>(account_hash, StorageEntry { key: slot_hash, value })
                .map_err(db_err)?;
        }
    }
    provider.commit().map_err(db_err)?;
    Ok(())
}

/// Writes a batch of bytecodes.
pub(crate) fn write_bytecodes<F>(factory: &F, codes: &[(B256, Bytes)]) -> Result<(), SnapSyncError>
where
    F: DatabaseProviderFactory,
    F::ProviderRW: DBProvider,
    <F::ProviderRW as DBProvider>::Tx: DbTxMut,
{
    let provider = factory.database_provider_rw().map_err(db_err)?;
    {
        let tx = provider.tx_ref();
        for (hash, code) in codes {
            if !code.is_empty() {
                tx.put::<tables::Bytecodes>(*hash, Bytecode::new_raw(code.clone()))
                    .map_err(db_err)?;
            }
        }
    }
    provider.commit().map_err(db_err)?;
    Ok(())
}

/// Writes stage checkpoints for all stages that snap sync satisfies.
///
/// After BAL healing completes, the database state corresponds to `target_block`.
/// This records that fact so the pipeline can resume from the correct point.
pub(crate) fn write_snap_stage_checkpoints<F>(
    factory: &F,
    target_block: u64,
) -> Result<(), SnapSyncError>
where
    F: DatabaseProviderFactory,
    F::ProviderRW: DBProvider + reth_provider::StaticFileProviderFactory,
    <F::ProviderRW as DBProvider>::Tx: DbTxMut,
{
    use reth_provider::{StaticFileProviderFactory, StaticFileSegment, StaticFileWriter};
    use reth_stages_types::{StageCheckpoint, StageId};

    let checkpoint = StageCheckpoint::new(target_block);
    let stages = [
        StageId::Bodies,
        StageId::SenderRecovery,
        StageId::Execution,
        StageId::AccountHashing,
        StageId::StorageHashing,
        StageId::TransactionLookup,
        StageId::IndexAccountHistory,
        StageId::IndexStorageHistory,
    ];

    let provider = factory.database_provider_rw().map_err(db_err)?;
    {
        let tx = provider.tx_ref();
        for stage_id in stages {
            tx.put::<tables::StageCheckpoints>(stage_id.to_string(), checkpoint).map_err(db_err)?;
        }
    }

    // Advance static file segments that snap sync did not populate (headers are
    // already filled by Phase A). Without this, the persistence service would fail
    // with `UnexpectedStaticFileBlockNumber` when writing blocks after the snap
    // target because these segments would still be at block 0.
    let segments = [
        StaticFileSegment::Transactions,
        StaticFileSegment::TransactionSenders,
        StaticFileSegment::Receipts,
        StaticFileSegment::AccountChangeSets,
        StaticFileSegment::StorageChangeSets,
    ];
    let sfp = provider.static_file_provider();
    for segment in segments {
        let mut writer = sfp.get_writer(0, segment).map_err(|e| {
            SnapSyncError::Database(format!("static file writer for {segment:?}: {e}"))
        })?;
        writer.ensure_at_block(target_block).map_err(|e| {
            SnapSyncError::Database(format!("ensure_at_block({target_block}) for {segment:?}: {e}"))
        })?;
    }
    sfp.commit().map_err(|e| {
        SnapSyncError::Database(format!("static file commit: {e}"))
    })?;

    provider.commit().map_err(db_err)?;
    Ok(())
}

/// Reads a single hashed account from the database.
pub(crate) fn read_hashed_account<F>(
    factory: &F,
    hashed_address: B256,
) -> Result<Option<Account>, SnapSyncError>
where
    F: DatabaseProviderFactory,
    F::Provider: DBProvider,
    <F::Provider as DBProvider>::Tx: DbTx,
{
    let provider = factory.database_provider_ro().map_err(db_err)?;
    let tx = provider.tx_ref();
    tx.get::<tables::HashedAccounts>(hashed_address).map_err(db_err)
}

// ──────────────────────────────────────────────────────────────────────────────
// RLP decoding helpers
// ──────────────────────────────────────────────────────────────────────────────

/// Decode a slim-format account from RLP bytes.
fn decode_slim_account(data: &[u8]) -> Result<Account, SnapSyncError> {
    let mut buf = data;
    let header = alloy_rlp::Header::decode(&mut buf)
        .map_err(|e| SnapSyncError::RlpDecode(format!("RLP error: {e}")))?;
    if !header.list {
        return Err(SnapSyncError::RlpDecode("expected RLP list for slim account".into()));
    }

    let nonce =
        u64::decode(&mut buf).map_err(|e| SnapSyncError::RlpDecode(format!("RLP error: {e}")))?;
    let balance =
        U256::decode(&mut buf).map_err(|e| SnapSyncError::RlpDecode(format!("RLP error: {e}")))?;
    let _storage_root =
        B256::decode(&mut buf).map_err(|e| SnapSyncError::RlpDecode(format!("RLP error: {e}")))?;
    let code_hash =
        B256::decode(&mut buf).map_err(|e| SnapSyncError::RlpDecode(format!("RLP error: {e}")))?;

    let bytecode_hash = if code_hash == KECCAK_EMPTY { None } else { Some(code_hash) };

    Ok(Account { nonce, balance, bytecode_hash })
}

/// Decode a storage value from RLP bytes (RLP-encoded [`U256`]).
fn decode_storage_value(data: &[u8]) -> Result<U256, SnapSyncError> {
    let mut buf = data;
    U256::decode(&mut buf).map_err(|e| SnapSyncError::RlpDecode(format!("RLP error: {e}")))
}

/// Increment a [`B256`] by 1 for pagination.
pub(crate) fn increment_b256(hash: B256) -> B256 {
    let mut bytes = hash.0;
    for byte in bytes.iter_mut().rev() {
        if *byte == 0xff {
            *byte = 0;
        } else {
            *byte += 1;
            return B256::from(bytes);
        }
    }
    B256::ZERO
}

fn db_err(e: impl std::error::Error + Send + Sync + 'static) -> SnapSyncError {
    SnapSyncError::Database(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_increment_b256_simple() {
        let hash = B256::ZERO;
        let next = increment_b256(hash);
        let mut expected = [0u8; 32];
        expected[31] = 1;
        assert_eq!(next, B256::from(expected));
    }

    #[test]
    fn test_increment_b256_carry() {
        let mut bytes = [0u8; 32];
        bytes[31] = 0xff;
        let hash = B256::from(bytes);
        let next = increment_b256(hash);
        let mut expected = [0u8; 32];
        expected[30] = 1;
        assert_eq!(next, B256::from(expected));
    }

    #[test]
    fn test_increment_b256_max() {
        let hash = B256::from([0xff; 32]);
        let next = increment_b256(hash);
        assert_eq!(next, B256::ZERO);
    }

    #[test]
    fn test_decode_slim_account_basic() {
        use alloy_rlp::Encodable;

        let nonce: u64 = 1;
        let balance = U256::from(100);
        let storage_root = B256::ZERO;
        let code_hash = KECCAK_EMPTY;

        let mut payload = Vec::new();
        nonce.encode(&mut payload);
        balance.encode(&mut payload);
        storage_root.encode(&mut payload);
        code_hash.encode(&mut payload);

        let mut buf = Vec::new();
        alloy_rlp::Header { list: true, payload_length: payload.len() }.encode(&mut buf);
        buf.extend_from_slice(&payload);

        let account = decode_slim_account(&buf).unwrap();
        assert_eq!(account.nonce, 1);
        assert_eq!(account.balance, U256::from(100));
        assert_eq!(account.bytecode_hash, None);
    }

    #[test]
    fn test_decode_slim_account_with_code() {
        use alloy_rlp::Encodable;

        let nonce: u64 = 5;
        let balance = U256::from(1000);
        let storage_root = B256::ZERO;
        let code_hash = B256::from([0xab; 32]);

        let mut payload = Vec::new();
        nonce.encode(&mut payload);
        balance.encode(&mut payload);
        storage_root.encode(&mut payload);
        code_hash.encode(&mut payload);

        let mut buf = Vec::new();
        alloy_rlp::Header { list: true, payload_length: payload.len() }.encode(&mut buf);
        buf.extend_from_slice(&payload);

        let account = decode_slim_account(&buf).unwrap();
        assert_eq!(account.nonce, 5);
        assert_eq!(account.balance, U256::from(1000));
        assert_eq!(account.bytecode_hash, Some(code_hash));
    }

    #[test]
    fn test_decode_storage_value() {
        use alloy_rlp::Encodable;

        let value = U256::from(42);
        let mut buf = Vec::new();
        value.encode(&mut buf);

        let decoded = decode_storage_value(&buf).unwrap();
        assert_eq!(decoded, U256::from(42));
    }
}

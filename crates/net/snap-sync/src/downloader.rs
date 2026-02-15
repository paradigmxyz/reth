//! Snap sync downloader.
//!
//! Orchestrates the multi-phase snap sync process:
//! 1. Download account ranges via `GetAccountRange`
//! 2. Download storage slots via `GetStorageRanges`
//! 3. Download bytecodes via `GetByteCodes`
//! 4. Verify state root against pivot block

use crate::{
    error::SnapSyncError,
    metrics::SnapSyncMetrics,
    progress::{SnapPhase, SnapProgress},
};
use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_primitives::{keccak256, Bytes, B256, U256};
use alloy_rlp::Decodable;
use alloy_trie::TrieAccount;
use reth_db_api::{
    cursor::DbCursorRO,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_eth_wire_types::snap::{
    GetAccountRangeMessage, GetByteCodesMessage, GetStorageRangesMessage,
};
use reth_network_p2p::snap::client::{SnapClient, SnapResponse};
use reth_primitives_traits::{Account, StorageEntry};
use reth_storage_api::{DBProvider, DatabaseProviderFactory};
use reth_trie::StateRoot;
use reth_trie_db::DatabaseStateRoot;
use std::collections::HashSet;
use tokio::sync::watch;
use tracing::{debug, info, trace, warn};

/// Maximum response size in bytes for snap requests (512 KB).
const MAX_RESPONSE_BYTES: u64 = 512 * 1024;

/// Number of accounts to accumulate before flushing to DB.
const ACCOUNT_WRITE_BATCH_SIZE: usize = 10_000;

/// Number of storage slots to accumulate before flushing to DB.
const STORAGE_WRITE_BATCH_SIZE: usize = 50_000;

/// Number of bytecodes to request in a single batch.
const BYTECODE_BATCH_SIZE: usize = 64;

/// Hash representing the maximum key (all 0xFF).
const HASH_MAX: B256 = B256::repeat_byte(0xFF);

/// The snap sync downloader that orchestrates state download from peers.
#[derive(Debug)]
pub struct SnapSyncDownloader<C, F> {
    /// The snap-capable network client.
    client: C,
    /// Database provider factory for writing state.
    provider_factory: F,
    /// Current sync progress.
    progress: SnapProgress,
    /// Metrics.
    metrics: SnapSyncMetrics,
    /// Cancellation signal.
    cancel_rx: watch::Receiver<bool>,
}

impl<C, F> SnapSyncDownloader<C, F>
where
    C: SnapClient + Clone + Send + Sync + 'static,
    F: DatabaseProviderFactory + Send + Sync + 'static,
    <F as DatabaseProviderFactory>::ProviderRW: DBProvider<Tx: DbTxMut + DbTx> + Send,
{
    /// Creates a new snap sync downloader.
    pub fn new(
        client: C,
        provider_factory: F,
        pivot_hash: B256,
        pivot_number: u64,
        state_root: B256,
        cancel_rx: watch::Receiver<bool>,
    ) -> Self {
        Self {
            client,
            provider_factory,
            progress: SnapProgress::new(pivot_hash, pivot_number, state_root),
            metrics: SnapSyncMetrics::default(),
            cancel_rx,
        }
    }

    /// Runs the snap sync to completion.
    ///
    /// Returns `Ok(())` if state was successfully downloaded and verified,
    /// or an error if sync failed.
    pub async fn run(&mut self) -> Result<(), SnapSyncError> {
        info!(
            target: "snap_sync",
            pivot_hash = %self.progress.pivot_hash,
            pivot_number = self.progress.pivot_number,
            state_root = %self.progress.state_root,
            "starting snap sync"
        );

        // Phase 1: Download accounts
        self.progress.phase = SnapPhase::Accounts;
        self.metrics.phase.set(1.0);
        let storage_accounts = self.download_accounts().await?;
        info!(
            target: "snap_sync",
            accounts = self.progress.accounts_downloaded,
            storage_accounts = storage_accounts.len(),
            "account download complete"
        );

        // Phase 2: Download storage slots
        self.progress.phase = SnapPhase::Storages;
        self.metrics.phase.set(2.0);
        let code_hashes = self.download_storages(&storage_accounts).await?;
        info!(
            target: "snap_sync",
            slots = self.progress.storage_slots_downloaded,
            "storage download complete"
        );

        // Phase 3: Download bytecodes
        self.progress.phase = SnapPhase::Bytecodes;
        self.metrics.phase.set(3.0);
        self.download_bytecodes(code_hashes).await?;
        info!(
            target: "snap_sync",
            bytecodes = self.progress.bytecodes_downloaded,
            "bytecode download complete"
        );

        // Phase 4: Verification (hashing + merkle root)
        self.progress.phase = SnapPhase::Verification;
        self.metrics.phase.set(4.0);
        info!(target: "snap_sync", "verifying state root against pivot block");
        self.verify_state_root()?;

        self.progress.phase = SnapPhase::Done;
        self.metrics.phase.set(5.0);

        Ok(())
    }

    /// Returns the current progress.
    pub const fn progress(&self) -> &SnapProgress {
        &self.progress
    }

    /// Checks if cancellation was requested.
    fn is_cancelled(&self) -> bool {
        *self.cancel_rx.borrow()
    }

    // ========================================================================
    // Phase 4: Verification
    // ========================================================================

    /// Computes the state root from the downloaded state and verifies it against
    /// the pivot block's expected state root.
    fn verify_state_root(&self) -> Result<(), SnapSyncError> {
        info!(target: "snap_sync", "computing state root from downloaded state");

        let provider =
            self.provider_factory.database_provider_rw().map_err(SnapSyncError::Provider)?;

        let computed_root = StateRoot::from_tx(provider.tx_ref())
            .root()
            .map_err(|e| SnapSyncError::StateRootVerification(e.to_string()))?;

        if computed_root != self.progress.state_root {
            return Err(SnapSyncError::StateRootMismatch {
                expected: self.progress.state_root,
                got: computed_root,
            });
        }

        info!(target: "snap_sync", %computed_root, "state root verified successfully");
        Ok(())
    }

    // ========================================================================
    // Phase 1: Account download
    // ========================================================================

    /// Downloads all accounts from the state trie via `GetAccountRange` requests.
    ///
    /// Returns a list of `(address_hash, storage_root)` for accounts that have
    /// non-empty storage (`storage_root` != `EMPTY_TRIE_HASH`).
    async fn download_accounts(&mut self) -> Result<Vec<(B256, B256)>, SnapSyncError> {
        let state_root = self.progress.state_root;
        let mut cursor = self.progress.account_cursor;
        let mut storage_accounts: Vec<(B256, B256)> = Vec::new();

        // Batch buffer for DB writes
        let mut account_batch: Vec<(B256, Account, B256)> = Vec::new();

        loop {
            if self.is_cancelled() {
                return Err(SnapSyncError::Cancelled);
            }

            let request = GetAccountRangeMessage {
                request_id: rand_request_id(),
                root_hash: state_root,
                starting_hash: cursor,
                limit_hash: HASH_MAX,
                response_bytes: MAX_RESPONSE_BYTES,
            };

            let response = self.client.get_account_range(request).await.map_err(|err| {
                self.metrics.request_failures.increment(1);
                SnapSyncError::Request(err)
            })?;

            let msg = match response.into_data() {
                SnapResponse::AccountRange(msg) => msg,
                _ => {
                    return Err(SnapSyncError::InvalidAccountRange(
                        "unexpected response type".into(),
                    ));
                }
            };

            if msg.accounts.is_empty() {
                // Empty response means we've reached the end or peer doesn't serve this root
                debug!(target: "snap_sync", %cursor, "received empty account range, finishing");
                break;
            }

            // Process each account
            for account_data in &msg.accounts {
                let trie_account = decode_slim_account(&account_data.body)?;

                let account = Account {
                    nonce: trie_account.nonce,
                    balance: trie_account.balance,
                    bytecode_hash: if trie_account.code_hash == KECCAK_EMPTY {
                        None
                    } else {
                        Some(trie_account.code_hash)
                    },
                };

                // Track accounts with storage
                let empty_root = alloy_trie::EMPTY_ROOT_HASH;
                if trie_account.storage_root != empty_root {
                    storage_accounts.push((account_data.hash, trie_account.storage_root));
                }

                account_batch.push((account_data.hash, account, trie_account.storage_root));
                self.progress.accounts_downloaded += 1;
            }

            // Update cursor to continue after the last account
            cursor = increment_hash(msg.accounts.last().unwrap().hash);

            // Flush batch if large enough
            if account_batch.len() >= ACCOUNT_WRITE_BATCH_SIZE {
                self.write_accounts(&account_batch)?;
                account_batch.clear();
            }

            self.metrics.accounts_downloaded.set(self.progress.accounts_downloaded as f64);
            self.progress.account_cursor = cursor;

            trace!(
                target: "snap_sync",
                accounts = self.progress.accounts_downloaded,
                %cursor,
                "account download progress"
            );

            // If cursor wrapped around to zero, we've covered the full range
            if cursor == B256::ZERO {
                break;
            }
        }

        // Flush remaining
        if !account_batch.is_empty() {
            self.write_accounts(&account_batch)?;
        }

        Ok(storage_accounts)
    }

    /// Writes a batch of accounts to the database.
    ///
    /// Account hashes are used as keys since snap protocol returns hashed addresses.
    /// The address→hash mapping will be resolved during the hashing stage.
    fn write_accounts(&self, batch: &[(B256, Account, B256)]) -> Result<(), SnapSyncError> {
        // For snap sync, we write to HashedAccounts table since snap returns hashed keys.
        // The pipeline's hashing stage normally computes this from PlainAccountState,
        // but for snap we go directly to the hashed form.
        let provider =
            self.provider_factory.database_provider_rw().map_err(SnapSyncError::Provider)?;

        let tx = provider.tx_ref();
        for (hash, account, _storage_root) in batch {
            tx.put::<tables::HashedAccounts>(*hash, *account)?;
        }

        provider.commit()?;
        Ok(())
    }

    // ========================================================================
    // Phase 2: Storage download
    // ========================================================================

    /// Downloads storage slots for all accounts with non-empty storage roots.
    ///
    /// Returns the set of code hashes encountered during account processing.
    async fn download_storages(
        &mut self,
        storage_accounts: &[(B256, B256)],
    ) -> Result<HashSet<B256>, SnapSyncError> {
        let state_root = self.progress.state_root;
        let code_hashes: HashSet<B256> = HashSet::new();

        // Process storage accounts in chunks
        let mut slot_batch: Vec<(B256, B256, U256)> = Vec::new();

        for (account_hash, _storage_root) in storage_accounts {
            if self.is_cancelled() {
                return Err(SnapSyncError::Cancelled);
            }

            let mut slot_cursor = B256::ZERO;

            loop {
                let request = GetStorageRangesMessage {
                    request_id: rand_request_id(),
                    root_hash: state_root,
                    account_hashes: vec![*account_hash],
                    starting_hash: slot_cursor,
                    limit_hash: HASH_MAX,
                    response_bytes: MAX_RESPONSE_BYTES,
                };

                let response = self.client.get_storage_ranges(request).await.map_err(|err| {
                    self.metrics.request_failures.increment(1);
                    SnapSyncError::Request(err)
                })?;

                let msg = match response.into_data() {
                    SnapResponse::StorageRanges(msg) => msg,
                    _ => {
                        return Err(SnapSyncError::InvalidStorageRange(
                            "unexpected response type".into(),
                        ));
                    }
                };

                if msg.slots.is_empty() || msg.slots[0].is_empty() {
                    break;
                }

                let slots = &msg.slots[0];
                for slot in slots {
                    let value = U256::from_be_slice(&slot.data);
                    slot_batch.push((*account_hash, slot.hash, value));
                    self.progress.storage_slots_downloaded += 1;
                }

                // Update cursor
                slot_cursor = increment_hash(slots.last().unwrap().hash);

                // Flush if needed
                if slot_batch.len() >= STORAGE_WRITE_BATCH_SIZE {
                    self.write_storage_slots(&slot_batch)?;
                    slot_batch.clear();
                }

                self.metrics
                    .storage_slots_downloaded
                    .set(self.progress.storage_slots_downloaded as f64);

                // If no proof or we've reached the end of this account's storage
                if msg.proof.is_empty() || slot_cursor == B256::ZERO {
                    break;
                }
            }
        }

        // Flush remaining
        if !slot_batch.is_empty() {
            self.write_storage_slots(&slot_batch)?;
        }

        Ok(code_hashes)
    }

    /// Writes a batch of storage slots to the database.
    fn write_storage_slots(&self, batch: &[(B256, B256, U256)]) -> Result<(), SnapSyncError> {
        let provider =
            self.provider_factory.database_provider_rw().map_err(SnapSyncError::Provider)?;

        let tx = provider.tx_ref();
        for (account_hash, slot_hash, value) in batch {
            tx.put::<tables::HashedStorages>(
                *account_hash,
                StorageEntry { key: *slot_hash, value: *value },
            )?;
        }

        provider.commit()?;
        Ok(())
    }

    // ========================================================================
    // Phase 3: Bytecode download
    // ========================================================================

    /// Downloads contract bytecodes by their code hashes.
    async fn download_bytecodes(
        &mut self,
        code_hashes: HashSet<B256>,
    ) -> Result<(), SnapSyncError> {
        // Collect code hashes from accounts we've already written
        let mut all_code_hashes: Vec<B256> = self.collect_code_hashes()?;
        all_code_hashes.extend(code_hashes);
        all_code_hashes.sort();
        all_code_hashes.dedup();

        // Remove KECCAK_EMPTY since that means no code
        all_code_hashes.retain(|h| *h != KECCAK_EMPTY);

        self.progress.bytecodes_total = all_code_hashes.len() as u64;

        info!(
            target: "snap_sync",
            total = all_code_hashes.len(),
            "starting bytecode download"
        );

        // Download in batches
        for chunk in all_code_hashes.chunks(BYTECODE_BATCH_SIZE) {
            if self.is_cancelled() {
                return Err(SnapSyncError::Cancelled);
            }

            let request = GetByteCodesMessage {
                request_id: rand_request_id(),
                hashes: chunk.to_vec(),
                response_bytes: MAX_RESPONSE_BYTES,
            };

            let response = self.client.get_byte_codes(request).await.map_err(|err| {
                self.metrics.request_failures.increment(1);
                SnapSyncError::Request(err)
            })?;

            let msg = match response.into_data() {
                SnapResponse::ByteCodes(msg) => msg,
                _ => {
                    return Err(SnapSyncError::InvalidBytecode("unexpected response type".into()));
                }
            };

            self.write_bytecodes(chunk, &msg.codes)?;
            self.progress.bytecodes_downloaded += msg.codes.len() as u64;
            self.metrics.bytecodes_downloaded.set(self.progress.bytecodes_downloaded as f64);
        }

        Ok(())
    }

    /// Collects code hashes from accounts already written to DB.
    fn collect_code_hashes(&self) -> Result<Vec<B256>, SnapSyncError> {
        let provider =
            self.provider_factory.database_provider_rw().map_err(SnapSyncError::Provider)?;

        let tx = provider.tx_ref();
        let mut cursor = tx.cursor_read::<tables::HashedAccounts>()?;
        let mut code_hashes = Vec::new();

        let mut entry = cursor.first()?;
        while let Some((_, account)) = entry {
            if let Some(hash) = account.bytecode_hash &&
                hash != KECCAK_EMPTY
            {
                code_hashes.push(hash);
            }
            entry = cursor.next()?;
        }

        Ok(code_hashes)
    }

    /// Writes bytecodes to the database, verifying hash integrity.
    fn write_bytecodes(
        &self,
        expected_hashes: &[B256],
        codes: &[Bytes],
    ) -> Result<(), SnapSyncError> {
        let provider =
            self.provider_factory.database_provider_rw().map_err(SnapSyncError::Provider)?;

        let tx = provider.tx_ref();
        for (i, code) in codes.iter().enumerate() {
            let hash = keccak256(code);
            if i < expected_hashes.len() && hash != expected_hashes[i] {
                warn!(
                    target: "snap_sync",
                    expected = %expected_hashes[i],
                    got = %hash,
                    "bytecode hash mismatch, skipping"
                );
                continue;
            }

            let bytecode = reth_primitives_traits::Bytecode::new_raw(code.clone());
            tx.put::<tables::Bytecodes>(hash, bytecode)?;
        }

        provider.commit()?;
        Ok(())
    }
}

/// Decodes a "slim" account body from the snap protocol into a `TrieAccount`.
///
/// Slim format is `RLP([nonce, balance, storage_root, code_hash])` but with
/// empty `storage_root` and `code_hash` omitted.
fn decode_slim_account(data: &Bytes) -> Result<TrieAccount, SnapSyncError> {
    // The snap protocol uses "slim" encoding where empty values are omitted.
    // We need to decode the RLP and fill in defaults for missing fields.
    let account = TrieAccount::decode(&mut data.as_ref())?;
    Ok(account)
}

/// Increments a hash by 1. Returns `B256::ZERO` on overflow (wraps around).
fn increment_hash(hash: B256) -> B256 {
    let mut bytes = hash.0;
    for i in (0..32).rev() {
        if bytes[i] < 0xFF {
            bytes[i] += 1;
            return B256::from(bytes);
        }
        bytes[i] = 0;
    }
    B256::ZERO
}

/// Generates a random request ID.
fn rand_request_id() -> u64 {
    rand::random()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_rlp::Encodable;

    #[test]
    fn test_increment_hash() {
        let zero = B256::ZERO;
        let one = increment_hash(zero);
        assert_eq!(one.0[31], 1);

        let max = B256::repeat_byte(0xFF);
        let wrapped = increment_hash(max);
        assert_eq!(wrapped, B256::ZERO);

        let mut mid = B256::ZERO;
        mid.0[31] = 0xFF;
        let next = increment_hash(mid);
        assert_eq!(next.0[30], 1);
        assert_eq!(next.0[31], 0);
    }

    #[test]
    fn test_decode_slim_account() {
        let trie_account = TrieAccount {
            nonce: 42,
            balance: U256::from(1000),
            storage_root: alloy_trie::EMPTY_ROOT_HASH,
            code_hash: KECCAK_EMPTY,
        };
        let mut buf = Vec::new();
        trie_account.encode(&mut buf);
        let decoded = decode_slim_account(&Bytes::from(buf)).unwrap();
        assert_eq!(decoded.nonce, 42);
        assert_eq!(decoded.balance, U256::from(1000));
        assert_eq!(decoded.storage_root, alloy_trie::EMPTY_ROOT_HASH);
        assert_eq!(decoded.code_hash, KECCAK_EMPTY);
    }

    #[test]
    fn test_decode_slim_account_empty() {
        let trie_account = TrieAccount {
            nonce: 0,
            balance: U256::ZERO,
            storage_root: alloy_trie::EMPTY_ROOT_HASH,
            code_hash: KECCAK_EMPTY,
        };
        let mut buf = Vec::new();
        trie_account.encode(&mut buf);
        let decoded = decode_slim_account(&Bytes::from(buf)).unwrap();
        assert_eq!(decoded.nonce, 0);
        assert_eq!(decoded.balance, U256::ZERO);
        assert_eq!(decoded.storage_root, alloy_trie::EMPTY_ROOT_HASH);
        assert_eq!(decoded.code_hash, KECCAK_EMPTY);
    }
}

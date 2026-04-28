//! Snap sync utilities for E2E tests.
//!
//! Provides a [`DbSnapStateProvider`] serving snap requests backed by MDBX.

use alloy_consensus::{constants::KECCAK_EMPTY, EMPTY_ROOT_HASH};
use alloy_primitives::{Bytes, B256};
use alloy_rlp::Encodable;
use reth_db::tables;
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    transaction::DbTx,
};
use reth_network::{
    snap_requests::SnapStateProvider,
    types::snap::{AccountData, StorageData},
};
use reth_primitives_traits::{Account, StorageEntry};
use reth_provider::DatabaseProviderFactory;
use reth_storage_api::DBProvider;
use reth_trie::StateRoot;
use reth_trie_db::DatabaseHashedCursorFactory;

/// Maximum accounts to return per request.
const MAX_ACCOUNTS_SERVE: usize = 4096;

/// Snap state provider backed by a [`DatabaseProviderFactory`].
///
/// Iterates `HashedAccounts` / `HashedStorages` / `Bytecodes` tables directly
/// via MDBX cursors. Computes state root from hashed leaf data (for tests).
pub struct DbSnapStateProvider<F> {
    factory: F,
}

impl<F> DbSnapStateProvider<F> {
    /// Create a new provider.
    pub fn new(factory: F) -> Self {
        Self { factory }
    }
}

impl<F> std::fmt::Debug for DbSnapStateProvider<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbSnapStateProvider").finish()
    }
}

/// Encode an [`Account`] as slim-format RLP: `[nonce, balance, storage_root, code_hash]`.
fn encode_slim_account(account: &Account) -> Bytes {
    let code_hash = account.bytecode_hash.unwrap_or(KECCAK_EMPTY);

    let mut payload = Vec::new();
    account.nonce.encode(&mut payload);
    account.balance.encode(&mut payload);
    EMPTY_ROOT_HASH.encode(&mut payload);
    code_hash.encode(&mut payload);

    let mut buf = Vec::new();
    alloy_rlp::Header { list: true, payload_length: payload.len() }.encode(&mut buf);
    buf.extend_from_slice(&payload);
    Bytes::from(buf)
}

impl<F> SnapStateProvider for DbSnapStateProvider<F>
where
    F: DatabaseProviderFactory + Send + Sync + 'static,
    F::Provider: DBProvider,
    <F::Provider as DBProvider>::Tx: DbTx,
{
    fn current_state_root(&self) -> B256 {
        use reth_trie::trie_cursor::noop::NoopTrieCursorFactory;
        let Ok(provider) = self.factory.database_provider_ro() else {
            return B256::ZERO;
        };
        let tx = provider.tx_ref();
        StateRoot::new(NoopTrieCursorFactory::default(), DatabaseHashedCursorFactory::new(tx))
            .root()
            .unwrap_or(B256::ZERO)
    }

    fn account_range(
        &self,
        _root_hash: B256,
        starting_hash: B256,
        limit_hash: B256,
        response_bytes: u64,
    ) -> (Vec<AccountData>, Vec<Bytes>) {
        let Ok(provider) = self.factory.database_provider_ro() else {
            return (Vec::new(), Vec::new());
        };
        let tx = provider.tx_ref();

        let Ok(mut cursor) = tx.cursor_read::<tables::HashedAccounts>() else {
            return (Vec::new(), Vec::new());
        };

        let mut accounts = Vec::new();
        let mut total_bytes: u64 = 0;

        let entry = if starting_hash == B256::ZERO {
            cursor.first().ok().flatten()
        } else {
            cursor.seek(starting_hash).ok().flatten()
        };

        if let Some((hash, account)) = entry {
            if hash < limit_hash {
                let body = encode_slim_account(&account);
                total_bytes += body.len() as u64 + 32;
                accounts.push(AccountData { hash, body });
            }
        }

        while accounts.len() < MAX_ACCOUNTS_SERVE && total_bytes < response_bytes {
            match cursor.next().ok().flatten() {
                Some((hash, account)) if hash < limit_hash => {
                    let body = encode_slim_account(&account);
                    total_bytes += body.len() as u64 + 32;
                    accounts.push(AccountData { hash, body });
                }
                _ => break,
            }
        }

        (accounts, Vec::new())
    }

    fn storage_ranges(
        &self,
        _root_hash: B256,
        account_hashes: Vec<B256>,
        starting_hash: B256,
        limit_hash: B256,
        response_bytes: u64,
    ) -> (Vec<Vec<StorageData>>, Vec<Bytes>) {
        let Ok(provider) = self.factory.database_provider_ro() else {
            return (Vec::new(), Vec::new());
        };
        let tx = provider.tx_ref();

        let Ok(mut cursor) = tx.cursor_dup_read::<tables::HashedStorages>() else {
            return (Vec::new(), Vec::new());
        };

        let mut all_slots = Vec::new();
        let mut total_bytes: u64 = 0;

        for (i, account_hash) in account_hashes.iter().enumerate() {
            let mut slots = Vec::new();

            let start = if i == 0 { starting_hash } else { B256::ZERO };

            let entry = if start == B256::ZERO {
                cursor.seek_exact(*account_hash).ok().flatten().map(|(_, v)| v)
            } else {
                cursor.seek_by_key_subkey(*account_hash, start).ok().flatten()
            };

            if let Some(StorageEntry { key, value }) = entry {
                if key < limit_hash && !value.is_zero() {
                    let mut data_buf = Vec::new();
                    value.encode(&mut data_buf);
                    let data = Bytes::from(data_buf);
                    total_bytes += data.len() as u64 + 32;
                    slots.push(StorageData { hash: key, data });
                }
            }

            while total_bytes < response_bytes {
                match cursor.next_dup_val().ok().flatten() {
                    Some(StorageEntry { key, value }) if key < limit_hash => {
                        if value.is_zero() {
                            continue;
                        }
                        let mut data_buf = Vec::new();
                        value.encode(&mut data_buf);
                        let data = Bytes::from(data_buf);
                        total_bytes += data.len() as u64 + 32;
                        slots.push(StorageData { hash: key, data });
                    }
                    _ => break,
                }
            }

            all_slots.push(slots);

            if total_bytes >= response_bytes {
                break;
            }
        }

        (all_slots, Vec::new())
    }

    fn bytecodes(&self, hashes: Vec<B256>, response_bytes: u64) -> Vec<Bytes> {
        let Ok(provider) = self.factory.database_provider_ro() else {
            return Vec::new();
        };
        let tx = provider.tx_ref();

        let mut codes = Vec::new();
        let mut total_bytes: u64 = 0;

        for hash in hashes {
            if let Ok(Some(bytecode)) = tx.get::<tables::Bytecodes>(hash) {
                let raw = Bytes::from(bytecode.bytes().to_vec());
                total_bytes += raw.len() as u64;
                codes.push(raw);
                if total_bytes >= response_bytes {
                    break;
                }
            } else {
                codes.push(Bytes::new());
            }
        }

        codes
    }
}

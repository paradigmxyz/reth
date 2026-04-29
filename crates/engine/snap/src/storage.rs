//! MDBX read/write helpers for hashed state, bytecodes, and RLP decoding utilities.

use crate::SnapSyncError;
use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_primitives::{Bytes, B256, U256};
use alloy_rlp::Decodable;
use reth_db_api::{
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives_traits::{Account, Bytecode, StorageEntry};
use reth_provider::DatabaseProviderFactory;
use reth_storage_api::DBProvider;

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

// ──────────────────────────────────────────────────────────────────────────────
// RLP decoding helpers
// ──────────────────────────────────────────────────────────────────────────────

/// Decode a slim-format account from RLP bytes.
pub(crate) fn decode_slim_account(data: &[u8]) -> Result<Account, SnapSyncError> {
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
pub(crate) fn decode_storage_value(data: &[u8]) -> Result<U256, SnapSyncError> {
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

pub(crate) fn db_err(e: impl std::error::Error + Send + Sync + 'static) -> SnapSyncError {
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

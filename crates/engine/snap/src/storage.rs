//! MDBX read/write helpers for hashed state and bytecodes.

use crate::SnapSyncError;
use alloy_primitives::{map::B256Map, Bytes, B256, U256};
use reth_db_api::{
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives_traits::{Account, Bytecode};
use reth_provider::DatabaseProviderFactory;
use reth_storage_api::{DBProvider, StateWriter};
use reth_trie::{HashedPostStateSorted, HashedStorageSorted};

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
        tx.clear::<tables::Bytecodes>().map_err(db_err)?;
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
    F::ProviderRW: DBProvider + StateWriter,
    <F::ProviderRW as DBProvider>::Tx: DbTxMut,
{
    let mut accounts_by_hash = B256Map::default();
    for (hash, account) in accounts {
        accounts_by_hash.insert(*hash, Some(*account));
    }

    let mut accounts: Vec<_> = accounts_by_hash.into_iter().collect();
    accounts.sort_by_key(|(hash, _)| *hash);

    write_hashed_state(factory, HashedPostStateSorted::new(accounts, B256Map::default()))
}

/// Writes a batch of hashed storage entries.
pub(crate) fn write_hashed_storages<F>(
    factory: &F,
    entries: &[(B256, B256, U256)],
) -> Result<(), SnapSyncError>
where
    F: DatabaseProviderFactory,
    F::ProviderRW: DBProvider + StateWriter,
    <F::ProviderRW as DBProvider>::Tx: DbTxMut,
{
    let mut slots_by_account: B256Map<B256Map<U256>> = B256Map::default();
    for &(account_hash, slot_hash, value) in entries {
        slots_by_account.entry(account_hash).or_default().insert(slot_hash, value);
    }

    let storages = slots_by_account
        .into_iter()
        .map(|(account_hash, slots)| {
            let mut storage_slots: Vec<_> = slots.into_iter().collect();
            storage_slots.sort_by_key(|(slot_hash, _)| *slot_hash);
            (account_hash, HashedStorageSorted { storage_slots, wiped: false })
        })
        .collect();

    write_hashed_state(factory, HashedPostStateSorted::new(Vec::new(), storages))
}

fn write_hashed_state<F>(
    factory: &F,
    hashed_state: HashedPostStateSorted,
) -> Result<(), SnapSyncError>
where
    F: DatabaseProviderFactory,
    F::ProviderRW: DBProvider + StateWriter,
    <F::ProviderRW as DBProvider>::Tx: DbTxMut,
{
    let provider = factory.database_provider_rw().map_err(db_err)?;
    provider.write_hashed_state(&hashed_state).map_err(db_err)?;
    provider.commit().map_err(db_err)?;
    Ok(())
}

/// Writes a batch of bytecodes.
pub(crate) fn write_bytecodes<F>(factory: &F, codes: &[(B256, Bytes)]) -> Result<(), SnapSyncError>
where
    F: DatabaseProviderFactory,
    F::ProviderRW: DBProvider + StateWriter,
    <F::ProviderRW as DBProvider>::Tx: DbTxMut,
{
    let provider = factory.database_provider_rw().map_err(db_err)?;
    provider
        .write_bytecodes(
            codes
                .iter()
                .filter(|(_, code)| !code.is_empty())
                .map(|(hash, code)| (*hash, Bytecode::new_raw(code.clone()))),
        )
        .map_err(db_err)?;
    provider.commit().map_err(db_err)?;
    Ok(())
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
}

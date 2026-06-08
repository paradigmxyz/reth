use alloy_primitives::{keccak256, B256};
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables,
    transaction::DbTx,
};
use reth_primitives_traits::{Account, StorageEntry};
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use reth_trie::lattice::{LatticeAccumulatorUpdates, LatticeHashState, LatticeRoot};
use revm_database::BundleState;
use std::io::{Error as IoError, ErrorKind};

const LATTICE_STATE_ACCUMULATOR_KEY: u8 = 0;

/// Computes a lattice state root from an EVM bundle state.
pub(crate) fn lattice_state_root<TX>(
    tx: &TX,
    bundle_state: &BundleState,
) -> ProviderResult<(B256, LatticeAccumulatorUpdates)>
where
    TX: DbTx,
{
    let mut lattice_root = state_accumulator(tx)?;

    let mut accounts = bundle_state.state.iter().collect::<Vec<_>>();
    accounts.sort_unstable_by_key(|(address, _)| keccak256(address));

    for (address, account) in accounts {
        let hashed_address = keccak256(address);
        let old_account: Option<Account> = account.original_info.as_ref().map(Into::into);
        let new_account: Option<Account> = account.info.as_ref().map(Into::into);

        if old_account != new_account {
            if let Some(old_account) = old_account {
                lattice_root.subtract_account(hashed_address, old_account);
            }
            if let Some(new_account) = new_account {
                lattice_root.add_account(hashed_address, new_account);
            }
        }

        if account.was_destroyed() {
            subtract_existing_storage(tx, &mut lattice_root, hashed_address)?;
            for (slot, value) in &account.storage {
                let present_value = value.present_value();
                if !present_value.is_zero() {
                    lattice_root.add_slot(
                        hashed_address,
                        keccak256(slot.to_be_bytes::<32>()),
                        present_value,
                    );
                }
            }
        } else {
            for (slot, value) in &account.storage {
                let original_value = value.original_value();
                let present_value = value.present_value();
                if original_value == present_value {
                    continue
                }

                let hashed_slot = keccak256(slot.to_be_bytes::<32>());
                if !original_value.is_zero() {
                    lattice_root.subtract_slot(hashed_address, hashed_slot, original_value);
                }
                if !present_value.is_zero() {
                    lattice_root.add_slot(hashed_address, hashed_slot, present_value);
                }
            }
        }
    }

    let root = lattice_root.root();
    let updates = LatticeAccumulatorUpdates::new(lattice_root.state());
    Ok((root, updates))
}

/// Rebuilds lattice accumulators from the current hashed state tables.
pub(crate) fn rebuild_lattice_accumulators<TX>(tx: &TX) -> ProviderResult<LatticeAccumulatorUpdates>
where
    TX: DbTx,
{
    let lattice_root = build_lattice_accumulator(tx)?;
    Ok(LatticeAccumulatorUpdates::new(lattice_root.state()))
}

/// Returns the lattice accumulator seed for incremental updates.
pub(crate) fn lattice_accumulator_seed<TX>(tx: &TX) -> ProviderResult<LatticeAccumulatorUpdates>
where
    TX: DbTx,
{
    let lattice_root = state_accumulator(tx)?;
    Ok(LatticeAccumulatorUpdates::new(lattice_root.state()))
}

/// Returns nonzero hashed storage entries for `hashed_address`.
pub(crate) fn lattice_account_storage<TX>(
    tx: &TX,
    hashed_address: B256,
) -> ProviderResult<Vec<(B256, alloy_primitives::U256)>>
where
    TX: DbTx,
{
    let mut storage = Vec::new();
    let mut cursor = tx.cursor_dup_read::<tables::HashedStorages>()?;

    for entry in cursor.walk_dup(Some(hashed_address), None)? {
        let (_, StorageEntry { key, value }) = entry?;
        if !value.is_zero() {
            storage.push((key, value));
        }
    }

    Ok(storage)
}

fn state_accumulator<TX>(tx: &TX) -> ProviderResult<LatticeRoot>
where
    TX: DbTx,
{
    if let Some(state) = tx.get::<tables::LatticeStateAccumulator>(LATTICE_STATE_ACCUMULATOR_KEY)? &&
        !has_legacy_storage_accumulators(tx)?
    {
        return LatticeHashState::from_slice(&state)
            .and_then(|state| LatticeRoot::from_state(&state))
            .map_err(lattice_decode_error)
    }

    let updates = rebuild_lattice_accumulators(tx)?;
    LatticeRoot::from_state(&updates.state).map_err(lattice_decode_error)
}

fn has_legacy_storage_accumulators<TX>(tx: &TX) -> ProviderResult<bool>
where
    TX: DbTx,
{
    let mut cursor = tx.cursor_read::<tables::LatticeStorageAccumulators>()?;
    Ok(cursor.first()?.is_some())
}

fn build_lattice_accumulator<TX>(tx: &TX) -> ProviderResult<LatticeRoot>
where
    TX: DbTx,
{
    let mut lattice_root = LatticeRoot::default();
    let mut account_cursor = tx.cursor_read::<tables::HashedAccounts>()?;
    let mut account_entry = account_cursor.seek(B256::ZERO)?;

    while let Some((hashed_address, account)) = account_entry {
        lattice_root.add_account(hashed_address, account);
        add_existing_storage(tx, &mut lattice_root, hashed_address)?;
        account_entry = account_cursor.next()?;
    }

    Ok(lattice_root)
}

fn add_existing_storage<TX>(
    tx: &TX,
    lattice_root: &mut LatticeRoot,
    hashed_address: B256,
) -> ProviderResult<()>
where
    TX: DbTx,
{
    let mut cursor = tx.cursor_dup_read::<tables::HashedStorages>()?;

    for entry in cursor.walk_dup(Some(hashed_address), None)? {
        let (_, StorageEntry { key, value }) = entry?;
        if !value.is_zero() {
            lattice_root.add_slot(hashed_address, key, value);
        }
    }

    Ok(())
}

fn subtract_existing_storage<TX>(
    tx: &TX,
    lattice_root: &mut LatticeRoot,
    hashed_address: B256,
) -> ProviderResult<()>
where
    TX: DbTx,
{
    let mut cursor = tx.cursor_dup_read::<tables::HashedStorages>()?;

    for entry in cursor.walk_dup(Some(hashed_address), None)? {
        let (_, StorageEntry { key, value }) = entry?;
        if !value.is_zero() {
            lattice_root.subtract_slot(hashed_address, key, value);
        }
    }

    Ok(())
}

pub(crate) fn lattice_decode_error(err: &'static str) -> ProviderError {
    ProviderError::other(IoError::new(ErrorKind::InvalidData, err))
}

pub(crate) const fn lattice_state_accumulator_key() -> u8 {
    LATTICE_STATE_ACCUMULATOR_KEY
}

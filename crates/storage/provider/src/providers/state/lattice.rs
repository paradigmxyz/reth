use alloy_primitives::{keccak256, map::B256Map, B256};
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables,
    transaction::DbTx,
};
use reth_primitives_traits::{Account, StorageEntry};
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use reth_trie::{
    lattice::{LatticeAccumulatorUpdates, LatticeHashState, LatticeStateRoot, LatticeStorageRoot},
    updates::TrieUpdates,
    HashedPostState, StateRoot,
};
use reth_trie_db::{
    DatabaseHashedCursorFactory, DatabaseStateRoot, DatabaseTrieCursorFactory, TrieTableAdapter,
};
use revm_database::BundleState;
use std::io::{Error as IoError, ErrorKind};

const LATTICE_STATE_ACCUMULATOR_KEY: u8 = 0;

type DbStateRoot<'a, TX, A> =
    StateRoot<DatabaseTrieCursorFactory<&'a TX, A>, DatabaseHashedCursorFactory<&'a TX>>;

/// Computes a lattice state root from an EVM bundle state and returns trie updates to persist.
pub(crate) fn lattice_state_root_with_updates<TX, A>(
    tx: &TX,
    bundle_state: &BundleState,
    hashed_state: HashedPostState,
    precomputed_trie_updates: Option<TrieUpdates>,
) -> ProviderResult<(B256, TrieUpdates, LatticeAccumulatorUpdates)>
where
    TX: DbTx,
    A: TrieTableAdapter,
{
    let trie_updates = match precomputed_trie_updates {
        Some(updates) => updates,
        None => {
            let sorted = hashed_state.into_sorted();
            <DbStateRoot<'_, TX, A> as DatabaseStateRoot<'_, TX>>::overlay_root_with_updates(
                tx, &sorted,
            )?
            .1
        }
    };

    let (mut state_root, mut storage_updates) = state_accumulator(tx)?;

    let mut accounts = bundle_state.state.iter().collect::<Vec<_>>();
    accounts.sort_unstable_by_key(|(address, _)| keccak256(address));

    for (address, account) in accounts {
        let hashed_address = keccak256(address);
        let old_account: Option<Account> = account.original_info.as_ref().map(Into::into);
        let new_account: Option<Account> = account.info.as_ref().map(Into::into);

        let mut storage_root = storage_accumulator(tx, hashed_address)?;
        let old_storage_root = storage_root.root();

        if account.was_destroyed() {
            storage_root.reset();
            for (slot, value) in &account.storage {
                let present_value = value.present_value();
                if !present_value.is_zero() {
                    storage_root.add_slot(keccak256(slot.to_be_bytes::<32>()), present_value);
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
                    storage_root.subtract_slot(hashed_slot, original_value);
                }
                if !present_value.is_zero() {
                    storage_root.add_slot(hashed_slot, present_value);
                }
            }
        }

        let new_storage_root = storage_root.root();
        if old_account != new_account || old_storage_root != new_storage_root {
            if let Some(old_account) = old_account {
                state_root.subtract_account(hashed_address, old_account, old_storage_root);
            }
            if let Some(new_account) = new_account {
                state_root.add_account(hashed_address, new_account, new_storage_root);
            }
        }

        if old_storage_root != new_storage_root || account.was_destroyed() {
            storage_updates
                .insert(hashed_address, (!storage_root.is_zero()).then_some(storage_root.state()));
        }
    }

    let root = state_root.root();
    let updates = LatticeAccumulatorUpdates::new(state_root.state(), storage_updates);
    Ok((root, trie_updates, updates))
}

/// Rebuilds lattice accumulators from the current hashed state tables.
pub(crate) fn rebuild_lattice_accumulators<TX>(tx: &TX) -> ProviderResult<LatticeAccumulatorUpdates>
where
    TX: DbTx,
{
    let state_root = build_state_accumulator(tx)?;
    let mut storage = B256Map::default();

    let mut account_cursor = tx.cursor_read::<tables::HashedAccounts>()?;
    let mut account_entry = account_cursor.seek(B256::ZERO)?;
    while let Some((hashed_address, _)) = account_entry {
        let storage_root = build_storage_accumulator(tx, hashed_address)?;
        storage.insert(hashed_address, (!storage_root.is_zero()).then_some(storage_root.state()));
        account_entry = account_cursor.next()?;
    }

    Ok(LatticeAccumulatorUpdates::new(state_root.state(), storage))
}

fn state_accumulator<TX>(
    tx: &TX,
) -> ProviderResult<(LatticeStateRoot, B256Map<Option<LatticeHashState>>)>
where
    TX: DbTx,
{
    if let Some(state) = tx.get::<tables::LatticeStateAccumulator>(LATTICE_STATE_ACCUMULATOR_KEY)? {
        let state_root = LatticeHashState::from_slice(&state)
            .and_then(|state| LatticeStateRoot::from_state(&state))
            .map_err(lattice_decode_error)?;
        return Ok((state_root, B256Map::default()))
    }

    let updates = rebuild_lattice_accumulators(tx)?;
    let state_root = LatticeStateRoot::from_state(&updates.state).map_err(lattice_decode_error)?;
    Ok((state_root, updates.storage))
}

fn storage_accumulator<TX>(tx: &TX, hashed_address: B256) -> ProviderResult<LatticeStorageRoot>
where
    TX: DbTx,
{
    if let Some(state) = tx.get::<tables::LatticeStorageAccumulators>(hashed_address)? {
        return LatticeHashState::from_slice(&state)
            .and_then(|state| LatticeStorageRoot::from_state(&state))
            .map_err(lattice_decode_error)
    }

    build_storage_accumulator(tx, hashed_address)
}

fn build_state_accumulator<TX>(tx: &TX) -> ProviderResult<LatticeStateRoot>
where
    TX: DbTx,
{
    let mut state_root = LatticeStateRoot::default();
    let mut account_cursor = tx.cursor_read::<tables::HashedAccounts>()?;
    let mut account_entry = account_cursor.seek(B256::ZERO)?;

    while let Some((hashed_address, account)) = account_entry {
        let storage_root = build_storage_accumulator(tx, hashed_address)?;
        state_root.add_account(hashed_address, account, storage_root.root());
        account_entry = account_cursor.next()?;
    }

    Ok(state_root)
}

fn build_storage_accumulator<TX>(
    tx: &TX,
    hashed_address: B256,
) -> ProviderResult<LatticeStorageRoot>
where
    TX: DbTx,
{
    let mut storage_root = LatticeStorageRoot::default();
    let mut cursor = tx.cursor_dup_read::<tables::HashedStorages>()?;

    for entry in cursor.walk_dup(Some(hashed_address), None)? {
        let (_, StorageEntry { key, value }) = entry?;
        if !value.is_zero() {
            storage_root.add_slot(key, value);
        }
    }

    Ok(storage_root)
}

pub(crate) fn lattice_decode_error(err: &'static str) -> ProviderError {
    ProviderError::other(IoError::new(ErrorKind::InvalidData, err))
}

pub(crate) const fn lattice_state_accumulator_key() -> u8 {
    LATTICE_STATE_ACCUMULATOR_KEY
}

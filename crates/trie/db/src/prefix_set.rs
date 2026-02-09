use alloy_primitives::{
    map::{HashMap, HashSet},
    BlockNumber, B256,
};
use core::ops::RangeInclusive;
use reth_db_api::{
    cursor::DbCursorRO,
    models::{AccountBeforeTx, BlockNumberAddress},
    tables,
    transaction::DbTx,
};
use reth_primitives_traits::StorageEntry;
use reth_storage_api::{ChangeSetReader, DBProvider, StorageChangeSetReader, StorageSettingsCache};
use reth_storage_errors::provider::ProviderError;
use reth_trie::{
    prefix_set::{PrefixSetMut, TriePrefixSets},
    IdentityKeyHasher, KeccakKeyHasher, KeyHasher, Nibbles,
};

/// Load prefix sets using a provider that implements [`ChangeSetReader`]. This function can read
/// changesets from both static files and database.
pub fn load_prefix_sets_with_provider<Provider, KH>(
    provider: &Provider,
    range: RangeInclusive<BlockNumber>,
) -> Result<TriePrefixSets, ProviderError>
where
    Provider: ChangeSetReader + StorageChangeSetReader + DBProvider,
    KH: KeyHasher,
{
    let tx = provider.tx_ref();

    // Initialize prefix sets.
    let mut account_prefix_set = PrefixSetMut::default();
    let mut storage_prefix_sets = HashMap::<B256, PrefixSetMut>::default();
    let mut destroyed_accounts = HashSet::default();

    // Get account changesets using the provider (handles static files + database)
    let account_changesets = provider.account_changesets_range(*range.start()..*range.end() + 1)?;

    // We still need direct access to HashedAccounts table
    let mut account_hashed_state_cursor = tx.cursor_read::<tables::HashedAccounts>()?;

    for (_, AccountBeforeTx { address, .. }) in account_changesets {
        let hashed_address = KH::hash_key(address);
        account_prefix_set.insert(Nibbles::unpack(hashed_address));

        if account_hashed_state_cursor.seek_exact(hashed_address)?.is_none() {
            destroyed_accounts.insert(hashed_address);
        }
    }

    // Walk storage changesets using the provider (handles static files + database)
    let storage_changesets = provider.storage_changesets_range(range)?;
    for (BlockNumberAddress((_, address)), StorageEntry { key, .. }) in storage_changesets {
        let hashed_address = KH::hash_key(address);
        account_prefix_set.insert(Nibbles::unpack(hashed_address));
        storage_prefix_sets
            .entry(hashed_address)
            .or_default()
            .insert(Nibbles::unpack(KH::hash_key(key)));
    }

    Ok(TriePrefixSets {
        account_prefix_set: account_prefix_set.freeze(),
        storage_prefix_sets: storage_prefix_sets
            .into_iter()
            .map(|(k, v)| (k, v.freeze()))
            .collect(),
        destroyed_accounts,
    })
}

/// Calls [`load_prefix_sets_with_provider`] with the correct [`KeyHasher`] based on storage settings.
pub fn load_prefix_sets_auto(
    provider: &(impl ChangeSetReader + StorageChangeSetReader + DBProvider + StorageSettingsCache),
    range: RangeInclusive<BlockNumber>,
) -> Result<TriePrefixSets, ProviderError> {
    if provider.cached_storage_settings().use_hashed_state {
        load_prefix_sets_with_provider::<_, IdentityKeyHasher>(provider, range)
    } else {
        load_prefix_sets_with_provider::<_, KeccakKeyHasher>(provider, range)
    }
}

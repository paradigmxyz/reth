use crate::{
    ChainSpecProvider, DBProvider, EitherWriter, HistoryWriter, NodePrimitivesProvider,
    ProviderResult, RocksDBProviderFactory, StorageSettingsCache,
};
use alloy_consensus::BlockHeader;
use alloy_genesis::GenesisAccount;
use alloy_primitives::Address;
use reth_chainspec::EthChainSpec;
use reth_db::{
    models::{storage_sharded_key::StorageShardedKey, ShardedKey},
    transaction::DbTxMut,
    BlockNumberList,
};
use tracing::trace;

/// Inserts history indices for genesis accounts and storage.
///
/// Writes to either MDBX or `RocksDB` based on storage settings configuration,
/// using [`EitherWriter`] to abstract over the storage backend.
pub fn insert_genesis_history<'a, 'b, Provider>(
    provider: &Provider,
    alloc: impl Iterator<Item = (&'a Address, &'b GenesisAccount)> + Clone,
) -> ProviderResult<()>
where
    Provider: DBProvider<Tx: DbTxMut>
        + HistoryWriter
        + ChainSpecProvider
        + StorageSettingsCache
        + RocksDBProviderFactory
        + NodePrimitivesProvider,
{
    let genesis_block_number = provider.chain_spec().genesis_header().number();
    insert_history(provider, alloc, genesis_block_number)
}

/// Inserts account history indices for genesis accounts.
pub fn insert_genesis_account_history<'a, 'b, Provider>(
    provider: &Provider,
    alloc: impl Iterator<Item = (&'a Address, &'b GenesisAccount)>,
) -> ProviderResult<()>
where
    Provider: DBProvider<Tx: DbTxMut>
        + HistoryWriter
        + ChainSpecProvider
        + StorageSettingsCache
        + RocksDBProviderFactory
        + NodePrimitivesProvider,
{
    let genesis_block_number = provider.chain_spec().genesis_header().number();
    insert_account_history(provider, alloc, genesis_block_number)
}

/// Inserts storage history indices for genesis accounts.
pub fn insert_genesis_storage_history<'a, 'b, Provider>(
    provider: &Provider,
    alloc: impl Iterator<Item = (&'a Address, &'b GenesisAccount)>,
) -> ProviderResult<()>
where
    Provider: DBProvider<Tx: DbTxMut>
        + HistoryWriter
        + ChainSpecProvider
        + StorageSettingsCache
        + RocksDBProviderFactory
        + NodePrimitivesProvider,
{
    let genesis_block_number = provider.chain_spec().genesis_header().number();
    insert_storage_history(provider, alloc, genesis_block_number)
}

/// Inserts history indices for genesis accounts and storage.
///
/// Writes to either MDBX or `RocksDB` based on storage settings configuration,
/// using [`EitherWriter`] to abstract over the storage backend.
pub fn insert_history<'a, 'b, Provider>(
    provider: &Provider,
    alloc: impl Iterator<Item = (&'a Address, &'b GenesisAccount)> + Clone,
    block: u64,
) -> ProviderResult<()>
where
    Provider: DBProvider<Tx: DbTxMut>
        + HistoryWriter
        + StorageSettingsCache
        + RocksDBProviderFactory
        + NodePrimitivesProvider,
{
    insert_account_history(provider, alloc.clone(), block)?;
    insert_storage_history(provider, alloc, block)?;
    Ok(())
}

/// Inserts account history indices at the given block.
pub fn insert_account_history<'a, 'b, Provider>(
    provider: &Provider,
    alloc: impl Iterator<Item = (&'a Address, &'b GenesisAccount)>,
    block: u64,
) -> ProviderResult<()>
where
    Provider: DBProvider<Tx: DbTxMut>
        + HistoryWriter
        + StorageSettingsCache
        + RocksDBProviderFactory
        + NodePrimitivesProvider,
{
    provider.with_rocksdb_batch(|batch| {
        let mut writer = EitherWriter::new_accounts_history(provider, batch)?;
        let list = BlockNumberList::new([block]).expect("single block always fits");
        for (addr, _) in alloc {
            writer.upsert_account_history(ShardedKey::last(*addr), &list)?;
        }
        trace!(target: "reth::provider", "Inserted account history");
        Ok(((), writer.into_raw_rocksdb_batch()))
    })?;

    Ok(())
}

/// Inserts storage history indices at the given block.
pub fn insert_storage_history<'a, 'b, Provider>(
    provider: &Provider,
    alloc: impl Iterator<Item = (&'a Address, &'b GenesisAccount)>,
    block: u64,
) -> ProviderResult<()>
where
    Provider: DBProvider<Tx: DbTxMut>
        + HistoryWriter
        + StorageSettingsCache
        + RocksDBProviderFactory
        + NodePrimitivesProvider,
{
    provider.with_rocksdb_batch(|batch| {
        let mut writer = EitherWriter::new_storages_history(provider, batch)?;
        let list = BlockNumberList::new([block]).expect("single block always fits");
        for (addr, account) in alloc {
            if let Some(storage) = &account.storage {
                for key in storage.keys() {
                    writer.upsert_storage_history(StorageShardedKey::last(*addr, *key), &list)?;
                }
            }
        }
        trace!(target: "reth::cli", "Inserted storage history");
        Ok(((), writer.into_raw_rocksdb_batch()))
    })?;

    Ok(())
}

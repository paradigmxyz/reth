use alloy_eips::{BlockId, BlockNumberOrTag};
use eyre::Result;
use reth_db::DatabaseEnv;
use reth_node_ethereum::EthereumNode;
use reth_node_types::NodeTypesWithDBAdapter;
use reth_provider::{
    BlockNumReader, DatabaseProviderFactory, ProviderError, ProviderFactory, StateProvider,
    TryIntoHistoricalStateProvider,
};
use std::{path::PathBuf, sync::Arc};

/// A tower-like layer that should be used as a [ProviderLayer](https://docs.rs/alloy/latest/alloy/providers/trait.ProviderLayer.html) in alloy when wrapping the
/// [Provider](https://docs.rs/alloy/latest/alloy/providers/trait.Provider.html) trait over reth-db.
#[derive(Debug, Clone)]
pub struct RethDbLayer {
    db_path: PathBuf,
}

impl RethDbLayer {
    /// Initialize the `RethDbLayer` with the path to the reth datadir.
    pub const fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }

    /// Get the provided path.
    pub const fn db_path(&self) -> &PathBuf {
        &self.db_path
    }
}

/// A helper type to get the appropriate DB provider.
#[derive(Debug, Clone)]
pub struct DbAccessor<DB = ProviderFactory<NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>>>
where
    DB: DatabaseProviderFactory<Provider: TryIntoHistoricalStateProvider + BlockNumReader>,
{
    inner: DB,
}

impl<DB> DbAccessor<DB>
where
    DB: DatabaseProviderFactory<Provider: TryIntoHistoricalStateProvider + BlockNumReader>,
{
    /// Initialize the `DbAccessor` with the provided `DB` type.
    pub const fn new(inner: DB) -> Self {
        Self { inner }
    }

    /// Get a read-only provider.
    pub fn provider(&self) -> Result<DB::Provider, ProviderError> {
        self.inner.database_provider_ro()
    }

    /// Get a read-only provider with state at the specified `BlockId`.
    pub fn provider_at(&self, block_id: BlockId) -> Result<Box<dyn StateProvider>, ProviderError> {
        let provider = self.inner.database_provider_ro()?;

        let block_number = match block_id {
            BlockId::Hash(hash) => {
                if let Some(num) = provider.block_number(hash.into())? {
                    num
                } else {
                    return Err(ProviderError::BlockHashNotFound(hash.into()));
                }
            }
            BlockId::Number(BlockNumberOrTag::Number(num)) => num,
            _ => provider.best_block_number()?,
        };

        provider.try_into_history_at_block(block_number)
    }
}

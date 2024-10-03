use alloy_eips::{BlockId, BlockNumberOrTag};
use alloy_primitives::{Address, U64};
use alloy_provider::{Provider, ProviderCall, ProviderLayer, RootProvider, RpcWithBlock};
use alloy_rpc_client::NoParams;
use alloy_transport::{Transport, TransportErrorKind};
use eyre::Result;
use reth_chainspec::ChainSpecBuilder;
use reth_db::{open_db_read_only, DatabaseEnv};
use reth_node_ethereum::EthereumNode;
use reth_node_types::NodeTypesWithDBAdapter;
use reth_provider::{
    providers::StaticFileProvider, BlockNumReader, DatabaseProviderFactory, ProviderError,
    ProviderFactory, StateProvider, TryIntoHistoricalStateProvider,
};
use std::{marker::PhantomData, path::PathBuf, sync::Arc};

/// A tower-like layer that should be used as a [`ProviderLayer`] to wrap the [`Provider`] trait
/// over reth-db.
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

/// Implement the [`ProviderLayer`] trait for the [`RethDbLayer`] struct.
impl<P, T> ProviderLayer<P, T> for RethDbLayer
where
    P: Provider<T>,
    T: Transport + Clone,
{
    type Provider = RethDbProvider<P, T>;

    fn layer(&self, inner: P) -> Self::Provider {
        RethDbProvider::new(inner, self.db_path().clone())
    }
}

/// A provider that overrides the vanilla [`Provider`] trait to get results from the reth-db.
#[derive(Clone, Debug)]
pub struct RethDbProvider<P, T> {
    inner: P,
    db_path: PathBuf,
    accessor: DbAccessor,
    _pd: PhantomData<T>,
}

impl<P, T> RethDbProvider<P, T> {
    /// Create a new [`RethDbProvider`] instance.
    pub fn new(inner: P, db_path: PathBuf) -> Self {
        let db = open_db_read_only(&db_path, Default::default()).unwrap();
        let chain_spec = ChainSpecBuilder::mainnet().build();
        let static_file_provider =
            StaticFileProvider::read_only(db_path.join("static_files"), false).unwrap();

        let provider_factory =
            ProviderFactory::new(db.into(), chain_spec.into(), static_file_provider);

        let accessor = DbAccessor::new(provider_factory);
        Self { inner, db_path, accessor, _pd: PhantomData }
    }

    /// Get the underlying [`Provider`].
    pub const fn inner(&self) -> &P {
        &self.inner
    }

    /// Get the underlying [`DbAccessor`].
    pub const fn accessor(&self) -> &DbAccessor {
        &self.accessor
    }

    /// Get the DB Path
    pub fn db_path(&self) -> PathBuf {
        self.db_path.clone()
    }
}

/// Implement the [`Provider`] trait for the [`RethDbProvider`] struct.
///
/// This is where we override specific RPC methods to fetch from the reth-db.
impl<P, T> Provider<T> for RethDbProvider<P, T>
where
    P: Provider<T>,
    T: Transport + Clone,
{
    fn root(&self) -> &RootProvider<T> {
        self.inner().root()
    }

    /// Override the `get_block_number` method to fetch the latest block number from the reth-db.
    fn get_block_number(&self) -> ProviderCall<T, NoParams, U64, u64> {
        let provider = self.accessor().provider().map_err(TransportErrorKind::custom).unwrap();

        let best = provider.best_block_number().map_err(TransportErrorKind::custom);

        ProviderCall::ready(best)
    }

    /// Override the `get_transaction_count` method to fetch the transaction count of an address.
    ///
    /// [`RpcWithBlock`] uses [`ProviderCall`] under the hood.
    fn get_transaction_count(&self, address: Address) -> RpcWithBlock<T, Address, U64, u64> {
        let this = self.accessor().clone();
        RpcWithBlock::new_provider(move |block_id| {
            let provider = this.provider_at(block_id).map_err(TransportErrorKind::custom).unwrap();

            let maybe_acc =
                provider.basic_account(address).map_err(TransportErrorKind::custom).unwrap();

            let nonce = maybe_acc.map(|acc| acc.nonce).unwrap_or_default();

            ProviderCall::ready(Ok(nonce))
        })
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
    /// Initialize the [`DbAccessor`] with the provided `DB` type.
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

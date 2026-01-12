//! Contains the main provider types and traits for interacting with the blockchain's storage.

use reth_chainspec::EthereumHardforks;
use reth_db_api::table::Value;
use reth_node_types::{NodePrimitives, NodeTypes, NodeTypesWithDB};

mod database;
pub use database::*;

mod static_file;
pub use static_file::{
    StaticFileAccess, StaticFileJarProvider, StaticFileProvider, StaticFileProviderBuilder,
    StaticFileProviderRW, StaticFileProviderRWRefMut, StaticFileWriter,
};

mod state;
pub use state::{
    historical::{
        needs_prev_shard_check, HistoricalStateProvider, HistoricalStateProviderRef, HistoryInfo,
        LowestAvailableBlocks,
    },
    latest::{LatestStateProvider, LatestStateProviderRef},
    overlay::{OverlayStateProvider, OverlayStateProviderFactory},
};

mod consistent_view;
pub use consistent_view::{ConsistentDbView, ConsistentViewError};

mod blockchain_provider;
pub use blockchain_provider::BlockchainProvider;

mod consistent;
pub use consistent::ConsistentProvider;

// RocksDB currently only supported on Unix platforms
// Windows support is planned for future releases
#[cfg_attr(all(unix, feature = "rocksdb"), path = "rocksdb/mod.rs")]
#[cfg_attr(not(all(unix, feature = "rocksdb")), path = "rocksdb_stub.rs")]
pub(crate) mod rocksdb;

pub use rocksdb::{RocksDBBatch, RocksDBBuilder, RocksDBProvider, RocksTx};

/// Helper trait to bound [`NodeTypes`] so that combined with database they satisfy
/// [`ProviderNodeTypes`].
pub trait NodeTypesForProvider
where
    Self: NodeTypes<
        ChainSpec: EthereumHardforks,
        Storage: ChainStorage<Self::Primitives>,
        Primitives: NodePrimitives<SignedTx: Value, Receipt: Value, BlockHeader: Value>,
    >,
{
}

impl<T> NodeTypesForProvider for T where
    T: NodeTypes<
        ChainSpec: EthereumHardforks,
        Storage: ChainStorage<T::Primitives>,
        Primitives: NodePrimitives<SignedTx: Value, Receipt: Value, BlockHeader: Value>,
    >
{
}

/// Helper trait keeping common requirements of providers for [`NodeTypesWithDB`].
pub trait ProviderNodeTypes
where
    Self: NodeTypesForProvider + NodeTypesWithDB,
{
}
impl<T> ProviderNodeTypes for T where T: NodeTypesForProvider + NodeTypesWithDB {}

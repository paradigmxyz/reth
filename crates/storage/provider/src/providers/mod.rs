//! Contains the main provider types and traits for interacting with the blockchain's storage.

use alloy_primitives::{Address, StorageKey, StorageValue};
use reth_chainspec::EthereumHardforks;
use reth_db_api::table::Value;
use reth_node_types::{NodePrimitives, NodeTypes, NodeTypesWithDB};
use reth_primitives_traits::Account;
use reth_storage_errors::provider::ProviderResult;
use std::sync::OnceLock;

/// External canonical-tip account read callback.
pub type FlatAccountRead =
    dyn Fn(&Address) -> ProviderResult<Option<Account>> + Send + Sync + 'static;
/// External canonical-tip storage read callback.
pub type FlatStorageRead =
    dyn Fn(Address, StorageKey) -> ProviderResult<Option<StorageValue>> + Send + Sync + 'static;

/// Optional process-wide latest-state point-read callbacks for external state backends.
///
/// The engine's in-memory overlays remain layered above these reads. Historical state, proofs,
/// and trie-table access are intentionally unaffected.
pub struct FlatStateReads {
    /// Reads an account from the external canonical-tip state.
    pub account: Box<FlatAccountRead>,
    /// Reads a storage slot from the external canonical-tip state.
    pub storage: Box<FlatStorageRead>,
    /// Whether the external backend owns canonical state persistence.
    ///
    /// When enabled, Reth continues to persist block data, receipts, and bytecodes, but skips its
    /// plain state, changesets, hashed state, trie nodes, and history indices.
    pub owns_state_persistence: bool,
}

impl core::fmt::Debug for FlatStateReads {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FlatStateReads").finish_non_exhaustive()
    }
}

/// External latest-state read callbacks, installed at most once during node startup.
pub static FLAT_STATE_READS: OnceLock<FlatStateReads> = OnceLock::new();

mod database;
pub use database::*;

mod static_file;
pub use static_file::{
    StaticFileAccess, StaticFileJarProvider, StaticFileProvider, StaticFileProviderBuilder,
    StaticFileProviderRW, StaticFileProviderRWRefMut, StaticFileWriteCtx, StaticFileWriter,
};

mod state;
pub use state::{
    historical::{compute_history_rank, history_info, needs_prev_shard_check, HistoryInfo},
    latest::{LatestStateProvider, LatestStateProviderRef},
};

mod blockchain_provider;
pub use blockchain_provider::{BlockchainProvider, SNAPSHOT_STATE_RETENTION};

mod consistent;
pub use consistent::ConsistentProvider;

pub(crate) mod rocksdb;

pub use rocksdb::{
    PruneShardOutcome, PrunedIndices, RocksDBBatch, RocksDBBuilder, RocksDBIter, RocksDBProvider,
    RocksDBRawIter, RocksDBStats, RocksDBTableStats, RocksReadSnapshot, RocksTx,
};

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

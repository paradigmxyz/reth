//! Contains the main provider types and traits for interacting with the blockchain's storage.

use reth_chainspec::EthereumHardforks;
use reth_db::table::Value;
use reth_node_types::{FullNodePrimitives, NodeTypes, NodeTypesWithDB, NodeTypesWithEngine};
use reth_primitives::EthPrimitives;

mod database;
pub use database::*;

mod static_file;
pub use static_file::{
    StaticFileAccess, StaticFileJarProvider, StaticFileProvider, StaticFileProviderRW,
    StaticFileProviderRWRefMut, StaticFileWriter,
};

mod state;
pub use state::{
    historical::{HistoricalStateProvider, HistoricalStateProviderRef},
    latest::{LatestStateProvider, LatestStateProviderRef},
};

mod bundle_state_provider;
pub use bundle_state_provider::BundleStateProvider;

mod consistent_view;
pub use consistent_view::{ConsistentDbView, ConsistentViewError};

mod blockchain_provider;
pub use blockchain_provider::BlockchainProvider;

mod consistent;
pub use consistent::ConsistentProvider;

/// Helper trait to bound [`NodeTypes`] so that combined with database they satisfy
/// [`ProviderNodeTypes`].
pub trait NodeTypesForProvider
where
    Self: NodeTypes<
        ChainSpec: EthereumHardforks,
        Storage: ChainStorage<Self::Primitives>,
        Primitives: FullNodePrimitives<SignedTx: Value, Receipt: Value, BlockHeader: Value>,
    >,
{
}

impl<T> NodeTypesForProvider for T where
    T: NodeTypes<
        ChainSpec: EthereumHardforks,
        Storage: ChainStorage<T::Primitives>,
        Primitives: FullNodePrimitives<SignedTx: Value, Receipt: Value, BlockHeader: Value>,
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

/// A helper trait with requirements for [`NodeTypesForProvider`] to be used within legacy
/// blockchain tree.
pub trait NodeTypesForTree:
    NodeTypesForProvider<Primitives = EthPrimitives> + NodeTypesWithEngine
{
}
impl<T> NodeTypesForTree for T where
    T: NodeTypesForProvider<Primitives = EthPrimitives> + NodeTypesWithEngine
{
}

/// Helper trait expressing requirements for node types to be used in engine.
pub trait EngineNodeTypes: ProviderNodeTypes + NodeTypesWithEngine {}

impl<T> EngineNodeTypes for T where T: ProviderNodeTypes + NodeTypesWithEngine {}

/// Helper trait with requirements for [`ProviderNodeTypes`] to be used within legacy blockchain
/// tree.
pub trait TreeNodeTypes: ProviderNodeTypes + NodeTypesForTree {}
impl<T> TreeNodeTypes for T where T: ProviderNodeTypes + NodeTypesForTree {}

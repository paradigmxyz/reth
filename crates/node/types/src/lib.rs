//! Standalone crate for Reth configuration traits and builder types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

use core::{fmt::Debug, marker::PhantomData};
pub use reth_primitives_traits::{
    Block, BlockBody, FullBlock, FullNodePrimitives, FullReceipt, FullSignedTx, NodePrimitives,
};

use reth_chainspec::EthChainSpec;
use reth_db_api::{
    database_metrics::{DatabaseMetadata, DatabaseMetrics},
    Database,
};
use reth_engine_primitives::EngineTypes;
use reth_trie_db::StateCommitment;

/// The type that configures the essential types of an Ethereum-like node.
///
/// This includes the primitive types of a node and chain specification.
///
/// This trait is intended to be stateless and only define the types of the node.
pub trait NodeTypes: Send + Sync + Unpin + 'static {
    /// The node's primitive types, defining basic operations and structures.
    type Primitives: NodePrimitives;
    /// The type used for configuration of the EVM.
    type ChainSpec: EthChainSpec;
    /// The type used to perform state commitment operations.
    type StateCommitment: StateCommitment;
    /// The type responsible for writing chain primitives to storage.
    type Storage: Default + Send + Sync + Unpin + Debug + 'static;
}

/// The type that configures an Ethereum-like node with an engine for consensus.
pub trait NodeTypesWithEngine: NodeTypes {
    /// The node's engine types, defining the interaction with the consensus engine.
    type Engine: EngineTypes;
}

/// A helper trait that is downstream of the [`NodeTypesWithEngine`] trait and adds database to the
/// node.
///
/// Its types are configured by node internally and are not intended to be user configurable.
pub trait NodeTypesWithDB: NodeTypes {
    /// Underlying database type used by the node to store and retrieve data.
    type DB: Database + DatabaseMetrics + DatabaseMetadata + Clone + Unpin + 'static;
}

/// An adapter type combining [`NodeTypes`] and db into [`NodeTypesWithDB`].
#[derive(Debug)]
pub struct NodeTypesWithDBAdapter<Types, DB> {
    types: PhantomData<Types>,
    db: PhantomData<DB>,
}

impl<Types, DB> NodeTypesWithDBAdapter<Types, DB> {
    /// Create a new adapter with the configured types.
    pub fn new() -> Self {
        Self { types: Default::default(), db: Default::default() }
    }
}

impl<Types, DB> Default for NodeTypesWithDBAdapter<Types, DB> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Types, DB> Clone for NodeTypesWithDBAdapter<Types, DB> {
    fn clone(&self) -> Self {
        Self { types: self.types, db: self.db }
    }
}

impl<Types, DB> NodeTypes for NodeTypesWithDBAdapter<Types, DB>
where
    Types: NodeTypes,
    DB: Send + Sync + Unpin + 'static,
{
    type Primitives = Types::Primitives;
    type ChainSpec = Types::ChainSpec;
    type StateCommitment = Types::StateCommitment;
    type Storage = Types::Storage;
}

impl<Types, DB> NodeTypesWithEngine for NodeTypesWithDBAdapter<Types, DB>
where
    Types: NodeTypesWithEngine,
    DB: Send + Sync + Unpin + 'static,
{
    type Engine = Types::Engine;
}

impl<Types, DB> NodeTypesWithDB for NodeTypesWithDBAdapter<Types, DB>
where
    Types: NodeTypes,
    DB: Database + DatabaseMetrics + DatabaseMetadata + Clone + Unpin + 'static,
{
    type DB = DB;
}

/// A [`NodeTypes`] type builder.
#[derive(Debug)]
pub struct AnyNodeTypes<P = (), C = (), SC = (), S = ()>(
    PhantomData<P>,
    PhantomData<C>,
    PhantomData<SC>,
    PhantomData<S>,
);

impl<P, C, SC, S> Default for AnyNodeTypes<P, C, SC, S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P, C, SC, S> AnyNodeTypes<P, C, SC, S> {
    /// Creates a new instance of [`AnyNodeTypes`].
    pub const fn new() -> Self {
        Self(PhantomData, PhantomData, PhantomData, PhantomData)
    }

    /// Sets the `Primitives` associated type.
    pub const fn primitives<T>(self) -> AnyNodeTypes<T, C, SC, S> {
        AnyNodeTypes::new()
    }

    /// Sets the `ChainSpec` associated type.
    pub const fn chain_spec<T>(self) -> AnyNodeTypes<P, T, SC, S> {
        AnyNodeTypes::new()
    }

    /// Sets the `StateCommitment` associated type.
    pub const fn state_commitment<T>(self) -> AnyNodeTypes<P, C, T, S> {
        AnyNodeTypes::new()
    }

    /// Sets the `Storage` associated type.
    pub const fn storage<T>(self) -> AnyNodeTypes<P, C, SC, T> {
        AnyNodeTypes::new()
    }
}

impl<P, C, SC, S> NodeTypes for AnyNodeTypes<P, C, SC, S>
where
    P: NodePrimitives + Send + Sync + Unpin + 'static,
    C: EthChainSpec + 'static,
    SC: StateCommitment,
    S: Default + Send + Sync + Unpin + Debug + 'static,
{
    type Primitives = P;
    type ChainSpec = C;
    type StateCommitment = SC;
    type Storage = S;
}

/// A [`NodeTypesWithEngine`] type builder.
#[derive(Debug)]
pub struct AnyNodeTypesWithEngine<P = (), E = (), C = (), SC = (), S = ()> {
    /// Embedding the basic node types.
    _base: AnyNodeTypes<P, C, SC, S>,
    /// Phantom data for the engine.
    _engine: PhantomData<E>,
}

impl<P, E, C, SC, S> Default for AnyNodeTypesWithEngine<P, E, C, SC, S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P, E, C, SC, S> AnyNodeTypesWithEngine<P, E, C, SC, S> {
    /// Creates a new instance of [`AnyNodeTypesWithEngine`].
    pub const fn new() -> Self {
        Self { _base: AnyNodeTypes::new(), _engine: PhantomData }
    }

    /// Sets the `Primitives` associated type.
    pub const fn primitives<T>(self) -> AnyNodeTypesWithEngine<T, E, C, SC, S> {
        AnyNodeTypesWithEngine::new()
    }

    /// Sets the `Engine` associated type.
    pub const fn engine<T>(self) -> AnyNodeTypesWithEngine<P, T, C, SC, S> {
        AnyNodeTypesWithEngine::new()
    }

    /// Sets the `ChainSpec` associated type.
    pub const fn chain_spec<T>(self) -> AnyNodeTypesWithEngine<P, E, T, SC, S> {
        AnyNodeTypesWithEngine::new()
    }

    /// Sets the `StateCommitment` associated type.
    pub const fn state_commitment<T>(self) -> AnyNodeTypesWithEngine<P, E, C, T, S> {
        AnyNodeTypesWithEngine::new()
    }

    /// Sets the `Storage` associated type.
    pub const fn storage<T>(self) -> AnyNodeTypesWithEngine<P, E, C, SC, T> {
        AnyNodeTypesWithEngine::new()
    }
}

impl<P, E, C, SC, S> NodeTypes for AnyNodeTypesWithEngine<P, E, C, SC, S>
where
    P: NodePrimitives + Send + Sync + Unpin + 'static,
    E: EngineTypes + Send + Sync + Unpin,
    C: EthChainSpec + 'static,
    SC: StateCommitment,
    S: Default + Send + Sync + Unpin + Debug + 'static,
{
    type Primitives = P;
    type ChainSpec = C;
    type StateCommitment = SC;
    type Storage = S;
}

impl<P, E, C, SC, S> NodeTypesWithEngine for AnyNodeTypesWithEngine<P, E, C, SC, S>
where
    P: NodePrimitives + Send + Sync + Unpin + 'static,
    E: EngineTypes + Send + Sync + Unpin,
    C: EthChainSpec + 'static,
    SC: StateCommitment,
    S: Default + Send + Sync + Unpin + Debug + 'static,
{
    type Engine = E;
}

/// Helper adapter type for accessing [`NodePrimitives::BlockHeader`] on [`NodeTypes`].
pub type HeaderTy<N> = <<N as NodeTypes>::Primitives as NodePrimitives>::BlockHeader;

/// Helper adapter type for accessing [`NodePrimitives::BlockBody`] on [`NodeTypes`].
pub type BodyTy<N> = <<N as NodeTypes>::Primitives as NodePrimitives>::BlockBody;

/// Helper adapter type for accessing [`NodePrimitives::SignedTx`] on [`NodeTypes`].
pub type TxTy<N> = <<N as NodeTypes>::Primitives as NodePrimitives>::SignedTx;

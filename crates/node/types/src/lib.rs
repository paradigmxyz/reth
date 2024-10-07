//! Standalone crate for Reth configuration traits and builder types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use std::marker::PhantomData;

use reth_chainspec::EthChainSpec;
use reth_db_api::{
    database_metrics::{DatabaseMetadata, DatabaseMetrics},
    Database,
};
use reth_engine_primitives::EngineTypes;

/// Configures all the primitive types of the node.
// TODO(mattsse): this is currently a placeholder
pub trait NodePrimitives {}

// TODO(mattsse): Placeholder
impl NodePrimitives for () {}

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
}

/// The type that configures an Ethereum-like node with an engine for consensus.
pub trait NodeTypesWithEngine: NodeTypes {
    /// The node's engine types, defining the interaction with the consensus engine.
    type Engine: EngineTypes;
    // type Engine: EngineTypes;
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
#[derive(Default, Debug)]
pub struct AnyNodeTypes<P = (), C = ()>(PhantomData<P>, PhantomData<C>);

impl<P, C> AnyNodeTypes<P, C> {
    /// Sets the `Primitives` associated type.
    pub const fn primitives<T>(self) -> AnyNodeTypes<T, C> {
        AnyNodeTypes::<T, C>(PhantomData::<T>, PhantomData::<C>)
    }

    /// Sets the `ChainSpec` associated type.
    pub const fn chain_spec<T>(self) -> AnyNodeTypes<P, T> {
        AnyNodeTypes::<P, T>(PhantomData::<P>, PhantomData::<T>)
    }
}

impl<P, C> NodeTypes for AnyNodeTypes<P, C>
where
    P: NodePrimitives + Send + Sync + Unpin + 'static,
    C: EthChainSpec + 'static,
{
    type Primitives = P;
    type ChainSpec = C;
}

/// A [`NodeTypesWithEngine`] type builder.
#[derive(Default, Debug)]
pub struct AnyNodeTypesWithEngine<P = (), E = (), C = ()> {
    /// Embedding the basic node types.
    base: AnyNodeTypes<P, C>,
    /// Phantom data for the engine.
    _engine: PhantomData<E>,
}

impl<P, E, C> AnyNodeTypesWithEngine<P, E, C> {
    /// Sets the `Primitives` associated type.
    pub const fn primitives<T>(self) -> AnyNodeTypesWithEngine<T, E, C> {
        AnyNodeTypesWithEngine { base: self.base.primitives::<T>(), _engine: PhantomData }
    }

    /// Sets the `Engine` associated type.
    pub const fn engine<T>(self) -> AnyNodeTypesWithEngine<P, T, C> {
        AnyNodeTypesWithEngine { base: self.base, _engine: PhantomData::<T> }
    }

    /// Sets the `ChainSpec` associated type.
    pub const fn chain_spec<T>(self) -> AnyNodeTypesWithEngine<P, E, T> {
        AnyNodeTypesWithEngine { base: self.base.chain_spec::<T>(), _engine: PhantomData }
    }
}

impl<P, E, C> NodeTypes for AnyNodeTypesWithEngine<P, E, C>
where
    P: NodePrimitives + Send + Sync + Unpin + 'static,
    E: EngineTypes + Send + Sync + Unpin,
    C: EthChainSpec + 'static,
{
    type Primitives = P;
    type ChainSpec = C;
}

impl<P, E, C> NodeTypesWithEngine for AnyNodeTypesWithEngine<P, E, C>
where
    P: NodePrimitives + Send + Sync + Unpin + 'static,
    E: EngineTypes + Send + Sync + Unpin,
    C: EthChainSpec + 'static,
{
    type Engine = E;
}

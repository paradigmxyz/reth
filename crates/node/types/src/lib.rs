//! Standalone crate for Reth configuration traits and builder types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

pub use reth_primitives_traits::{Block, BlockBody, FullBlock, FullReceipt, FullSignedTx};

use core::{fmt, marker::PhantomData};

use reth_chainspec::EthChainSpec;
use reth_db_api::{
    database_metrics::{DatabaseMetadata, DatabaseMetrics},
    Database,
};
use reth_engine_primitives::EngineTypes;
use reth_trie_db::StateCommitment;

/// Configures all the primitive types of the node.
pub trait NodePrimitives: Send + Sync + Unpin + Clone + Default + fmt::Debug {
    /// Block primitive.
    type Block: Send + Sync + Unpin + Clone + Default + fmt::Debug + 'static;
    /// Signed version of the transaction type.
    type SignedTx: Send + Sync + Unpin + Clone + Default + fmt::Debug + 'static;
    /// A receipt.
    type Receipt: Send + Sync + Unpin + Clone + Default + fmt::Debug + 'static;
}

impl NodePrimitives for () {
    type Block = ();
    type SignedTx = ();
    type Receipt = ();
}

/// Helper trait that sets trait bounds on [`NodePrimitives`].
pub trait FullNodePrimitives: Send + Sync + Unpin + Clone + Default + fmt::Debug {
    /// Block primitive.
    type Block: FullBlock<Body: BlockBody<SignedTransaction = Self::SignedTx>>;
    /// Signed version of the transaction type.
    type SignedTx: FullSignedTx;
    /// A receipt.
    type Receipt: FullReceipt;
}

impl<T> NodePrimitives for T
where
    T: FullNodePrimitives<Block: 'static, SignedTx: 'static, Receipt: 'static>,
{
    type Block = T::Block;
    type SignedTx = T::SignedTx;
    type Receipt = T::Receipt;
}

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
pub struct AnyNodeTypes<P = (), C = (), S = ()>(PhantomData<P>, PhantomData<C>, PhantomData<S>);

impl<P, C, S> AnyNodeTypes<P, C, S> {
    /// Sets the `Primitives` associated type.
    pub const fn primitives<T>(self) -> AnyNodeTypes<T, C, S> {
        AnyNodeTypes::<T, C, S>(PhantomData::<T>, PhantomData::<C>, PhantomData::<S>)
    }

    /// Sets the `ChainSpec` associated type.
    pub const fn chain_spec<T>(self) -> AnyNodeTypes<P, T, S> {
        AnyNodeTypes::<P, T, S>(PhantomData::<P>, PhantomData::<T>, PhantomData::<S>)
    }

    /// Sets the `StateCommitment` associated type.
    pub const fn state_commitment<T>(self) -> AnyNodeTypes<P, C, T> {
        AnyNodeTypes::<P, C, T>(PhantomData::<P>, PhantomData::<C>, PhantomData::<T>)
    }
}

impl<P, C, S> NodeTypes for AnyNodeTypes<P, C, S>
where
    P: NodePrimitives + Send + Sync + Unpin + 'static,
    C: EthChainSpec + 'static,
    S: StateCommitment,
{
    type Primitives = P;
    type ChainSpec = C;
    type StateCommitment = S;
}

/// A [`NodeTypesWithEngine`] type builder.
#[derive(Default, Debug)]
pub struct AnyNodeTypesWithEngine<P = (), E = (), C = (), S = ()> {
    /// Embedding the basic node types.
    base: AnyNodeTypes<P, C, S>,
    /// Phantom data for the engine.
    _engine: PhantomData<E>,
}

impl<P, E, C, S> AnyNodeTypesWithEngine<P, E, C, S> {
    /// Sets the `Primitives` associated type.
    pub const fn primitives<T>(self) -> AnyNodeTypesWithEngine<T, E, C, S> {
        AnyNodeTypesWithEngine { base: self.base.primitives::<T>(), _engine: PhantomData }
    }

    /// Sets the `Engine` associated type.
    pub const fn engine<T>(self) -> AnyNodeTypesWithEngine<P, T, C, S> {
        AnyNodeTypesWithEngine { base: self.base, _engine: PhantomData::<T> }
    }

    /// Sets the `ChainSpec` associated type.
    pub const fn chain_spec<T>(self) -> AnyNodeTypesWithEngine<P, E, T, S> {
        AnyNodeTypesWithEngine { base: self.base.chain_spec::<T>(), _engine: PhantomData }
    }

    /// Sets the `StateCommitment` associated type.
    pub const fn state_commitment<T>(self) -> AnyNodeTypesWithEngine<P, E, C, T> {
        AnyNodeTypesWithEngine { base: self.base.state_commitment::<T>(), _engine: PhantomData }
    }
}

impl<P, E, C, S> NodeTypes for AnyNodeTypesWithEngine<P, E, C, S>
where
    P: NodePrimitives + Send + Sync + Unpin + 'static,
    E: EngineTypes + Send + Sync + Unpin,
    C: EthChainSpec + 'static,
    S: StateCommitment,
{
    type Primitives = P;
    type ChainSpec = C;
    type StateCommitment = S;
}

impl<P, E, C, S> NodeTypesWithEngine for AnyNodeTypesWithEngine<P, E, C, S>
where
    P: NodePrimitives + Send + Sync + Unpin + 'static,
    E: EngineTypes + Send + Sync + Unpin,
    C: EthChainSpec + 'static,
    S: StateCommitment,
{
    type Engine = E;
}

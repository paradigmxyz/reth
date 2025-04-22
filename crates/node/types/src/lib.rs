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
use reth_db_api::{database_metrics::DatabaseMetrics, Database};
use reth_engine_primitives::EngineTypes;
use reth_payload_primitives::{BuiltPayload, PayloadTypes};
use reth_trie_db::StateCommitment;

/// The type that configures the essential types of an Ethereum-like node.
///
/// This includes the primitive types of a node and chain specification.
///
/// This trait is intended to be stateless and only define the types of the node.
pub trait NodeTypes: Clone + Debug + Send + Sync + Unpin + 'static {
    /// The node's primitive types, defining basic operations and structures.
    type Primitives: NodePrimitives;
    /// The type used for configuration of the EVM.
    type ChainSpec: EthChainSpec<Header = <Self::Primitives as NodePrimitives>::BlockHeader>;
    /// The type used to perform state commitment operations.
    type StateCommitment: StateCommitment;
    /// The type responsible for writing chain primitives to storage.
    type Storage: Default + Send + Sync + Unpin + Debug + 'static;
    /// The node's engine types, defining the interaction with the consensus engine.
    type Payload: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = Self::Primitives>>;
}

/// A helper trait that is downstream of the [`NodeTypes`] trait and adds database to the
/// node.
///
/// Its types are configured by node internally and are not intended to be user configurable.
pub trait NodeTypesWithDB: NodeTypes {
    /// Underlying database type used by the node to store and retrieve data.
    type DB: Database + DatabaseMetrics + Clone + Unpin + 'static;
}

/// An adapter type combining [`NodeTypes`] and db into [`NodeTypesWithDB`].
#[derive(Clone, Debug, Default)]
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

impl<Types, DB> NodeTypes for NodeTypesWithDBAdapter<Types, DB>
where
    Types: NodeTypes,
    DB: Clone + Debug + Send + Sync + Unpin + 'static,
{
    type Primitives = Types::Primitives;
    type ChainSpec = Types::ChainSpec;
    type StateCommitment = Types::StateCommitment;
    type Storage = Types::Storage;
    type Payload = Types::Payload;
}

impl<Types, DB> NodeTypesWithDB for NodeTypesWithDBAdapter<Types, DB>
where
    Types: NodeTypes,
    DB: Database + DatabaseMetrics + Clone + Unpin + 'static,
{
    type DB = DB;
}

/// A [`NodeTypes`] type builder.
#[derive(Clone, Debug, Default)]
pub struct AnyNodeTypes<P = (), C = (), SC = (), S = (), PL = ()>(
    PhantomData<P>,
    PhantomData<C>,
    PhantomData<SC>,
    PhantomData<S>,
    PhantomData<PL>,
);

impl<P, C, SC, S, PL> AnyNodeTypes<P, C, SC, S, PL> {
    /// Creates a new instance of [`AnyNodeTypes`].
    pub const fn new() -> Self {
        Self(PhantomData, PhantomData, PhantomData, PhantomData, PhantomData)
    }

    /// Sets the `Primitives` associated type.
    pub const fn primitives<T>(self) -> AnyNodeTypes<T, C, SC, S, PL> {
        AnyNodeTypes::new()
    }

    /// Sets the `ChainSpec` associated type.
    pub const fn chain_spec<T>(self) -> AnyNodeTypes<P, T, SC, S, PL> {
        AnyNodeTypes::new()
    }

    /// Sets the `StateCommitment` associated type.
    pub const fn state_commitment<T>(self) -> AnyNodeTypes<P, C, T, S, PL> {
        AnyNodeTypes::new()
    }

    /// Sets the `Storage` associated type.
    pub const fn storage<T>(self) -> AnyNodeTypes<P, C, SC, T, PL> {
        AnyNodeTypes::new()
    }

    /// Sets the `Payload` associated type.
    pub const fn payload<T>(self) -> AnyNodeTypes<P, C, SC, S, T> {
        AnyNodeTypes::new()
    }
}

impl<P, C, SC, S, PL> NodeTypes for AnyNodeTypes<P, C, SC, S, PL>
where
    P: NodePrimitives + Send + Sync + Unpin + 'static,
    C: EthChainSpec<Header = P::BlockHeader> + Clone + 'static,
    SC: StateCommitment,
    S: Default + Clone + Send + Sync + Unpin + Debug + 'static,
    PL: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = P>> + Send + Sync + Unpin + 'static,
{
    type Primitives = P;
    type ChainSpec = C;
    type StateCommitment = SC;
    type Storage = S;
    type Payload = PL;
}

/// A [`NodeTypes`] type builder.
#[derive(Clone, Debug, Default)]
pub struct AnyNodeTypesWithEngine<P = (), E = (), C = (), SC = (), S = (), PL = ()> {
    /// Embedding the basic node types.
    _base: AnyNodeTypes<P, C, SC, S, PL>,
    /// Phantom data for the engine.
    _engine: PhantomData<E>,
}

impl<P, E, C, SC, S, PL> AnyNodeTypesWithEngine<P, E, C, SC, S, PL> {
    /// Creates a new instance of [`AnyNodeTypesWithEngine`].
    pub const fn new() -> Self {
        Self { _base: AnyNodeTypes::new(), _engine: PhantomData }
    }

    /// Sets the `Primitives` associated type.
    pub const fn primitives<T>(self) -> AnyNodeTypesWithEngine<T, E, C, SC, S, PL> {
        AnyNodeTypesWithEngine::new()
    }

    /// Sets the `Engine` associated type.
    pub const fn engine<T>(self) -> AnyNodeTypesWithEngine<P, T, C, SC, S, PL> {
        AnyNodeTypesWithEngine::new()
    }

    /// Sets the `ChainSpec` associated type.
    pub const fn chain_spec<T>(self) -> AnyNodeTypesWithEngine<P, E, T, SC, S, PL> {
        AnyNodeTypesWithEngine::new()
    }

    /// Sets the `StateCommitment` associated type.
    pub const fn state_commitment<T>(self) -> AnyNodeTypesWithEngine<P, E, C, T, S, PL> {
        AnyNodeTypesWithEngine::new()
    }

    /// Sets the `Storage` associated type.
    pub const fn storage<T>(self) -> AnyNodeTypesWithEngine<P, E, C, SC, T, PL> {
        AnyNodeTypesWithEngine::new()
    }

    /// Sets the `Payload` associated type.
    pub const fn payload<T>(self) -> AnyNodeTypesWithEngine<P, E, C, SC, S, T> {
        AnyNodeTypesWithEngine::new()
    }
}

impl<P, E, C, SC, S, PL> NodeTypes for AnyNodeTypesWithEngine<P, E, C, SC, S, PL>
where
    P: NodePrimitives + Send + Sync + Unpin + 'static,
    E: EngineTypes + Send + Sync + Unpin,
    C: EthChainSpec<Header = P::BlockHeader> + Clone + 'static,
    SC: StateCommitment,
    S: Default + Clone + Send + Sync + Unpin + Debug + 'static,
    PL: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = P>> + Send + Sync + Unpin + 'static,
{
    type Primitives = P;
    type ChainSpec = C;
    type StateCommitment = SC;
    type Storage = S;
    type Payload = PL;
}

/// Helper adapter type for accessing [`NodePrimitives::Block`] on [`NodeTypes`].
pub type BlockTy<N> = <PrimitivesTy<N> as NodePrimitives>::Block;

/// Helper adapter type for accessing [`NodePrimitives::BlockHeader`] on [`NodeTypes`].
pub type HeaderTy<N> = <PrimitivesTy<N> as NodePrimitives>::BlockHeader;

/// Helper adapter type for accessing [`NodePrimitives::BlockBody`] on [`NodeTypes`].
pub type BodyTy<N> = <PrimitivesTy<N> as NodePrimitives>::BlockBody;

/// Helper adapter type for accessing [`NodePrimitives::SignedTx`] on [`NodeTypes`].
pub type TxTy<N> = <PrimitivesTy<N> as NodePrimitives>::SignedTx;

/// Helper adapter type for accessing [`NodePrimitives::Receipt`] on [`NodeTypes`].
pub type ReceiptTy<N> = <PrimitivesTy<N> as NodePrimitives>::Receipt;

/// Helper type for getting the `Primitives` associated type from a [`NodeTypes`].
pub type PrimitivesTy<N> = <N as NodeTypes>::Primitives;

/// Helper type for getting the `Primitives` associated type from a [`NodeTypes`].
pub type KeyHasherTy<N> = <<N as NodeTypes>::StateCommitment as StateCommitment>::KeyHasher;

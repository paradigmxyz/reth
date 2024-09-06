use crate::{
    NodeStorage, NodeTypesWithStorage,
    NodeTypesWithStorageAdapter,
};
use reth_chainspec::EthChainSpec;
use reth_db::{
    test_utils::TempDatabase,
    DatabaseEnv,
};
use reth_engine_primitives::EngineTypes;
use reth_node_types::{
    AnyNodeTypes, AnyNodeTypesWithEngine, NodePrimitives, NodeTypes, NodeTypesWithEngine,
};
use std::{marker::PhantomData, sync::Arc};

/// Mock [`reth_node_types::NodeTypes`] for testing.
pub type MockNodeTypes = AnyNodeTypesWithStorage<
    (),
    reth_chainspec::ChainSpec,
    reth_ethereum_engine_primitives::EthEngineTypes,
    MockNodeStorage,
>;

/// Mock [`NodeStorage`].
#[derive(Debug)]
pub struct MockNodeStorage();

impl NodeStorage for MockNodeStorage {
    type Types = AnyNodeTypes<(), reth_chainspec::ChainSpec>;

    fn new() -> Self {
        todo!()
    }

    fn read_block<TX: reth_db::transaction::DbTx>(
        &self,
        _id: reth_primitives::BlockHashOrNumber,
        _provider: crate::DatabaseProvider<TX>,
    ) -> <<AnyNodeTypes<(), reth_chainspec::ChainSpec> as NodeTypes>::Primitives as NodePrimitives>::Block{
        todo!()
    }
}

/// A [`NodeTypesWithStorage`] type builder.
#[derive(Default, Debug)]
pub struct AnyNodeTypesWithStorage<P = (), C = (), E = (), S = ()> {
    /// Embedding the basic node types.
    base: AnyNodeTypesWithEngine<P, E, C>,
    /// Phantom data for the storage
    _storage: PhantomData<S>,
}

impl<P, C, E, S> AnyNodeTypesWithStorage<P, C, E, S> {
    /// Sets the `Primitives` associated type.
    pub const fn primitives<T>(self) -> AnyNodeTypesWithStorage<T, C, E, S> {
        AnyNodeTypesWithStorage { base: self.base.primitives::<T>(), _storage: PhantomData }
    }

    /// Sets the `ChainSpec` associated type.
    pub const fn chain_spec<T>(self) -> AnyNodeTypesWithStorage<P, T, E, S> {
        AnyNodeTypesWithStorage { base: self.base.chain_spec::<T>(), _storage: PhantomData }
    }

    /// Sets the `Engine` associated type.
    pub const fn engine<T>(self) -> AnyNodeTypesWithStorage<P, C, T, S> {
        AnyNodeTypesWithStorage { base: self.base.engine::<T>(), _storage: PhantomData }
    }

    /// Sets the `Storage` associated type.
    pub const fn storage<T>(self) -> AnyNodeTypesWithStorage<P, C, E, T> {
        AnyNodeTypesWithStorage { base: self.base, _storage: PhantomData::<T> }
    }
}

impl<P, C, E, S> NodeTypes for AnyNodeTypesWithStorage<P, C, E, S>
where
    P: NodePrimitives + Send + Sync + Unpin + 'static,
    C: EthChainSpec,
    E: EngineTypes + Send + Sync + Unpin + 'static,
    S: NodeStorage + Send + Sync + Unpin + 'static,
{
    type Primitives = P;
    type ChainSpec = C;
}

impl<P, C, E, S> NodeTypesWithEngine for AnyNodeTypesWithStorage<P, C, E, S>
where
    P: NodePrimitives + Send + Sync + Unpin + 'static,
    C: EthChainSpec,
    E: EngineTypes + Send + Sync + Unpin + 'static,
    S: NodeStorage + Send + Sync + Unpin + 'static,
{
    type Engine = E;
}

impl<P, C, E, S> NodeTypesWithStorage for AnyNodeTypesWithStorage<P, C, E, S>
where
    P: NodePrimitives + Send + Sync + Unpin + 'static,
    C: EthChainSpec,
    E: EngineTypes + Send + Sync + Unpin + 'static,
    S: NodeStorage + Send + Sync + Unpin + 'static,
{
    type Storage = S;
}

/// Mock [`reth_node_types::NodeTypesWithDB`] for testing.
pub type MockNodeTypesWithStorage<DB = TempDatabase<DatabaseEnv>> =
    NodeTypesWithStorageAdapter<MockNodeTypes, Arc<DB>>;

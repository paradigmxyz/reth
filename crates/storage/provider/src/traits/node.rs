use std::marker::PhantomData;

use reth_chainspec::ChainSpec;
use reth_db::{
    database_metrics::{DatabaseMetadata, DatabaseMetrics},
    transaction::DbTx,
    Database,
};
use reth_node_types::{NodePrimitives, NodeTypes, NodeTypesWithDB, NodeTypesWithEngine};
use reth_primitives::BlockHashOrNumber;

use crate::DatabaseProvider;

/// A helper trait that is downstream of the [`NodeTypes`] trait and adds [`NodeStorage`].
pub trait NodeTypesWithStorage: NodeTypes {
    /// The node's methods for reading and writing primitive types to storage.
    type Storage: NodeStorage;
}

/// Trait that defines how a node writes and reads its own specific [`NodeTypes`] to storage.
pub trait NodeStorage {
    /// The node's types
    type Types: NodeTypes;

    /// Returns a new instance of the type that implements [`NodeStorage`].
    fn new() -> Self;

    /// Reads block
    fn read_block<TX: DbTx>(
        &self,
        id: BlockHashOrNumber,
        provider: DatabaseProvider<TX>,
    ) -> <<Self::Types as NodeTypes>::Primitives as NodePrimitives>::Block;
}

/// An adapter type combining [`NodeTypesWithStorage`] and [`NodeTypesWithDB`].
#[derive(Debug)]
pub struct NodeTypesWithStorageAdapter<Types, DB> {
    types: PhantomData<Types>,
    db: PhantomData<DB>,
}

impl<Types, DB> NodeTypesWithStorageAdapter<Types, DB> {
    /// Create a new adapter with the configured types.
    pub fn new() -> Self {
        Self { types: Default::default(), db: Default::default() }
    }
}

impl<Types, DB> Default for NodeTypesWithStorageAdapter<Types, DB> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Types, DB> Clone for NodeTypesWithStorageAdapter<Types, DB> {
    fn clone(&self) -> Self {
        Self { types: self.types, db: self.db }
    }
}

impl<Types, DB> NodeTypes for NodeTypesWithStorageAdapter<Types, DB>
where
    Types: NodeTypes,
    DB: Send + Sync + Unpin + 'static,
{
    type Primitives = Types::Primitives;
    type ChainSpec = Types::ChainSpec;
}

impl<Types, DB> NodeTypesWithEngine for NodeTypesWithStorageAdapter<Types, DB>
where
    Types: NodeTypesWithEngine,
    DB: Send + Sync + Unpin + 'static,
{
    type Engine = Types::Engine;
}

impl<Types, DB> NodeTypesWithDB for NodeTypesWithStorageAdapter<Types, DB>
where
    Types: NodeTypes,
    DB: Database + DatabaseMetrics + DatabaseMetadata + Clone + Unpin + 'static,
{
    type DB = DB;
}

impl<Types, DB> NodeTypesWithStorage for NodeTypesWithStorageAdapter<Types, DB>
where
    Types: NodeTypesWithStorage,
    DB: Database + DatabaseMetrics + DatabaseMetadata + Clone + Unpin + 'static,
{
    type Storage = Types::Storage;
}

/// Helper trait keeping common requirements of providers for [`NodeTypesWithDB`].
pub trait ProviderNodeTypes: NodeTypesWithDB<ChainSpec = ChainSpec> + NodeTypesWithStorage {}

impl<T> ProviderNodeTypes for T where
    T: NodeTypesWithDB<ChainSpec = ChainSpec> + NodeTypesWithStorage
{
}

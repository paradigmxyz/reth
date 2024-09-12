use crate::{providers::StaticFileProvider, DatabaseProvider, DatabaseProviderRO};
use reth_chain_state::CanonicalInMemoryState;
use reth_db_api::{transaction::DbTx, Database};
use reth_node_types::NodeTypes;
use reth_primitives::{Block, BlockHashOrNumber};
use reth_prune_types::PruneModes;
use reth_storage_errors::provider::ProviderResult;

trait NodeStorage: Clone {
    type Node: NodeTypes;

    /// Reads block
    fn read_block<TX: DbTx>(
        &self,
        id: BlockHashOrNumber,
        provider: DatabaseProvider<TX>,
    ) -> ProviderResult<Option<Block>>;
}

/// A common provider that fetches data from a database or static file.
///
/// This provider implements most provider or provider factory traits.
///
/// TODO this acts a type that can be used to read and write data from storage, it provides access
/// to the database and static file provider. This manages `DatabaseProvider` and passes this to the
/// storage to read from and write to.
pub struct ProviderFactory2<DB, S> {
    /// Database
    db: DB,
    /// Static File Provider
    static_file_provider: StaticFileProvider,
    /// Optional pruning configuration
    prune_modes: PruneModes,
    /// How to read,write certain types of data
    storage: S,
}

impl<DB, S> ProviderFactory2<DB, S>
where
    DB: Database,
{
    pub fn provider(&self) -> ProviderResult<DatabaseProviderRO<DB>> {
        Ok(DatabaseProvider::new(
            self.db.tx()?,
            // TODO get rid of this: https://github.com/paradigmxyz/reth/issues/10854
            reth_chainspec::DEV.clone(),
            self.static_file_provider.clone(),
            self.prune_modes.clone(),
        ))
    }
}

impl<DB, S> ProviderFactory2<DB, S>
where
    DB: Database,
    S: NodeStorage,
{
    fn block(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Block>> {
        let provider = self.provider()?;
        let block = self.storage.read_block(id, provider)?;
        Ok(block)
    }
}

/// The main type for interacting with the blockchain.
///
/// This type serves as the main entry point for interacting with the blockchain and provides data
/// from database storage and from the blockchain tree (pending state etc.) It is a simple wrapper
/// type that holds an instance of the database and the blockchain tree.
#[derive(Debug, Clone)]
pub struct BlockchainProvider3<S> {
    /// Disk storage support for the blockchain.
    storage: S, // this will ProviderFactory
    /// Tracks the chain info wrt forkchoice updates and in memory canonical
    /// state.
    pub(super) canonical_in_memory_state: CanonicalInMemoryState,
}

// TODO implement all traits that storage also supports

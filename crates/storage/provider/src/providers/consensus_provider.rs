use crate::{ConsensusNumberReader, ConsensusNumberWriter, ProviderFactory};
use reth_db::database::Database;
use reth_interfaces::provider::ProviderResult;
use reth_primitives::{BlockNumber, B256};

/// The main type for interacting with the blockchain.
///
/// This type serves as the main entry point for interacting with the blockchain and provides data
/// from database storage and from the blockchain tree (pending state etc.) It is a simple wrapper
/// type that holds an instance of the database and the blockchain tree.
#[derive(Clone, Debug)]
pub struct ConsensusProvider<DB> {
    /// Provider type used to access the database.
    database: ProviderFactory<DB>,
}

impl<DB> ConsensusProvider<DB>
where
    DB: Database,
{
    /// Create a new provider using only the database and the tree, fetching the latest header from
    /// the database to initialize the provider.
    pub fn new(database: ProviderFactory<DB>) -> ProviderResult<Self> {
        Ok(Self { database })
    }
}

impl<DB> ConsensusNumberReader for ConsensusProvider<DB>
where
    DB: Database,
{
    /// Returns the best block number in the chain.
    fn last_consensus_number(&self) -> ProviderResult<BlockNumber> {
        self.database.provider()?.last_consensus_number()
    }

    /// Gets the `BlockNumber` for the given hash. Returns `None` if no block with this hash exists.
    fn consensus_number(&self, hash: B256) -> ProviderResult<Option<BlockNumber>> {
        self.database.provider()?.consensus_number(hash)
    }
}

impl<DB> ConsensusNumberWriter for ConsensusProvider<DB>
where
    DB: Database,
{
    fn save_consensus_number(&self, hash: B256, num: BlockNumber) -> ProviderResult<bool> {
        let provider = self.database.provider_rw()?;
        provider.save_consensus_number(hash, num)?;
        provider.commit()
    }
}

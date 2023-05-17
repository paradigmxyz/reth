use reth_primitives::{Address, BlockHash, BlockHashOrNumber, BlockNumber, TxNumber, H256};

/// Bundled errors variants thrown by various providers.
#[allow(missing_docs)]
#[derive(Debug, thiserror::Error, PartialEq, Eq, Clone)]
pub enum ProviderError {
    /// The header number was not found for the given block hash.
    #[error("Block hash {0:?} does not exist in Headers table")]
    BlockHashNotFound(BlockHash),
    /// A block body is missing.
    #[error("Block meta not found for block #{0}")]
    BlockBodyIndicesNotFound(BlockNumber),
    /// The transition id was found for the given address and storage key, but the changeset was
    /// not found.
    #[error("Storage ChangeSet address: ({address:?} key: {storage_key:?}) for block:#{block_number} does not exist")]
    StorageChangesetNotFound {
        /// The block number found for the address and storage key
        block_number: BlockNumber,
        /// The account address
        address: Address,
        /// The storage key
        storage_key: H256,
    },
    /// The block number was found for the given address, but the changeset was not found.
    #[error("Account {address:?} ChangeSet for block #{block_number} does not exist")]
    AccountChangesetNotFound {
        /// Block number found for the address
        block_number: BlockNumber,
        /// The account address
        address: Address,
    },
    /// The total difficulty for a block is missing.
    #[error("Total difficulty not found for block #{number}")]
    TotalDifficultyNotFound { number: BlockNumber },
    /// Thrown when required header related data was not found but was required.
    #[error("No header found for {0:?}")]
    HeaderNotFound(BlockHashOrNumber),
    /// Thrown we were unable to find the best block
    #[error("Best block does not exist")]
    BestBlockNotFound,
    /// Thrown we were unable to find the finalized block
    #[error("Finalized block does not exist")]
    FinalizedBlockNotFound,
    /// Thrown we were unable to find the safe block
    #[error("Safe block does not exist")]
    SafeBlockNotFound,
    /// Mismatch of sender and transaction
    #[error("Mismatch of sender and transaction id {tx_id}")]
    MismatchOfTransactionAndSenderId { tx_id: TxNumber },
    /// Block body wrong transaction count
    #[error("Stored block indices does not match transaction count")]
    BlockBodyTransactionCount,
    /// Thrown when the cache service task dropped
    #[error("cache service task stopped")]
    CacheServiceUnavailable,
    /// Thrown when we failed to lookup a block for the pending state
    #[error("Unknown block hash: {0:}")]
    UnknownBlockHash(H256),
    /// Thrown when we were unable to find a state for a block hash
    #[error("No State found for block hash: {0:}")]
    StateForHashNotFound(H256),
    /// Unable to compute state root on top of historical block
    #[error("Unable to compute state root on top of historical block")]
    StateRootNotAvailableForHistoricalBlock,
}

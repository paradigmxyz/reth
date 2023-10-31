use reth_primitives::{
    Address, BlockHash, BlockHashOrNumber, BlockNumber, TxHashOrNumber, TxNumber, B256,
};

/// Bundled errors variants thrown by various providers.
#[derive(Debug, thiserror::Error, PartialEq, Eq, Clone)]
pub enum ProviderError {
    /// Database error.
    #[error(transparent)]
    Database(#[from] crate::db::DatabaseError),
    /// The header number was not found for the given block hash.
    #[error("block hash {0:?} does not exist in Headers table")]
    BlockHashNotFound(BlockHash),
    /// A block body is missing.
    #[error("block meta not found for block #{0}")]
    BlockBodyIndicesNotFound(BlockNumber),
    /// The transition id was found for the given address and storage key, but the changeset was
    /// not found.
    #[error("storage ChangeSet address: ({address:?} key: {storage_key:?}) for block:#{block_number} does not exist")]
    StorageChangesetNotFound {
        /// The block number found for the address and storage key.
        block_number: BlockNumber,
        /// The account address.
        address: Address,
        /// The storage key.
        storage_key: B256,
    },
    /// The block number was found for the given address, but the changeset was not found.
    #[error("account {address:?} ChangeSet for block #{block_number} does not exist")]
    AccountChangesetNotFound {
        /// Block number found for the address.
        block_number: BlockNumber,
        /// The account address.
        address: Address,
    },
    /// The total difficulty for a block is missing.
    #[error("total difficulty not found for block #{block_number}")]
    TotalDifficultyNotFound {
        /// The block number.
        block_number: BlockNumber,
    },
    /// when required header related data was not found but was required.
    #[error("no header found for {0:?}")]
    HeaderNotFound(BlockHashOrNumber),
    /// The specific transaction is missing.
    #[error("no transaction found for {0:?}")]
    TransactionNotFound(TxHashOrNumber),
    /// The specific receipt is missing
    #[error("no receipt found for {0:?}")]
    ReceiptNotFound(TxHashOrNumber),
    /// Unable to find a specific block.
    #[error("block does not exist {0:?}")]
    BlockNotFound(BlockHashOrNumber),
    /// Unable to find the best block.
    #[error("best block does not exist")]
    BestBlockNotFound,
    /// Unable to find the finalized block.
    #[error("finalized block does not exist")]
    FinalizedBlockNotFound,
    /// Unable to find the safe block.
    #[error("safe block does not exist")]
    SafeBlockNotFound,
    /// Mismatch of sender and transaction.
    #[error("mismatch of sender and transaction id {tx_id}")]
    MismatchOfTransactionAndSenderId {
        /// The transaction ID.
        tx_id: TxNumber,
    },
    /// Block body wrong transaction count.
    #[error("stored block indices does not match transaction count")]
    BlockBodyTransactionCount,
    /// Thrown when the cache service task dropped.
    #[error("cache service task stopped")]
    CacheServiceUnavailable,
    /// Thrown when we failed to lookup a block for the pending state.
    #[error("unknown block {0}")]
    UnknownBlockHash(B256),
    /// Thrown when we were unable to find a state for a block hash.
    #[error("no state found for block {0}")]
    StateForHashNotFound(B256),
    /// Unable to compute state root on top of historical block.
    #[error("unable to compute state root on top of historical block")]
    StateRootNotAvailableForHistoricalBlock,
    /// Unable to find the block number for a given transaction index.
    #[error("unable to find the block number for a given transaction index")]
    BlockNumberForTransactionIndexNotFound,
    /// Root mismatch.
    #[error("merkle trie root mismatch at #{block_number} ({block_hash}): got {got}, expected {expected}")]
    StateRootMismatch {
        /// The expected root.
        expected: B256,
        /// The calculated root.
        got: B256,
        /// The block number.
        block_number: BlockNumber,
        /// The block hash.
        block_hash: BlockHash,
    },
    /// Root mismatch during unwind
    #[error("unwind merkle trie root mismatch at #{block_number} ({block_hash}): got {got}, expected {expected}")]
    UnwindStateRootMismatch {
        /// Expected root
        expected: B256,
        /// Calculated root
        got: B256,
        /// Target block number
        block_number: BlockNumber,
        /// Block hash
        block_hash: BlockHash,
    },
    /// State is not available for the given block number because it is pruned.
    #[error("state at block #{0} is pruned")]
    StateAtBlockPruned(BlockNumber),
}

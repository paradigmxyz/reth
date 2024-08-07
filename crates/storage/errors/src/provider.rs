use reth_primitives::{
    Address, BlockHash, BlockHashOrNumber, BlockNumber, GotExpected, StaticFileSegment,
    TxHashOrNumber, TxNumber, B256, U256,
};
use derive_more::{Display, From};
use alloc::{boxed::Box, string::String};
use crate::db::DatabaseError;

#[cfg(feature = "std")]
use std::path::PathBuf;

/// Provider result type.
pub type ProviderResult<Ok> = Result<Ok, ProviderError>;

/// Bundled errors variants thrown by various providers.
#[derive(Clone, Debug, Display, From, PartialEq, Eq)]
pub enum ProviderError {
    /// Database error.
    Database(DatabaseError),
    /// RLP error.
    Rlp(alloy_rlp::Error),
    /// Filesystem path error.
    #[from(ignore)]
    #[display(fmt = "{_0}")]
    FsPathError(String),
    /// Nippy jar error.
    #[from(ignore)]
    #[display(fmt = "nippy jar error: {_0}")]
    NippyJar(String),
    /// Trie witness error.
    #[from(ignore)]
    #[display(fmt = "trie witness error: {_0}")]
    TrieWitnessError(String),
    /// Error when recovering the sender for a transaction
    #[display(fmt = "failed to recover sender for transaction")]
    SenderRecoveryError,
    /// The header number was not found for the given block hash.
    #[from(ignore)]
    #[display(fmt = "block hash {_0} does not exist in Headers table")]
    BlockHashNotFound(BlockHash),
    /// A block body is missing.
    #[from(ignore)]
    #[display(fmt = "block meta not found for block #{_0}")]
    BlockBodyIndicesNotFound(BlockNumber),
    /// The transition ID was found for the given address and storage key, but the changeset was
    /// not found.
    #[from(ignore)]
    #[display(fmt = "storage change set for address {address} and key {storage_key} at block #{block_number} does not exist")]
    StorageChangesetNotFound {
        /// The block number found for the address and storage key.
        block_number: BlockNumber,
        /// The account address.
        address: Address,
        /// The storage key.
        // NOTE: This is a Box only because otherwise this variant is 16 bytes larger than the
        // second largest (which uses `BlockHashOrNumber`).
        storage_key: Box<B256>,
    },
    /// The block number was found for the given address, but the changeset was not found.
    #[from(ignore)]
    #[display(fmt = "account change set for address {address} at block #{block_number} does not exist")]
    AccountChangesetNotFound {
        /// Block number found for the address.
        block_number: BlockNumber,
        /// The account address.
        address: Address,
    },
    /// The total difficulty for a block is missing.
    #[from(ignore)]
    #[display(fmt = "total difficulty not found for block #{_0}")]
    TotalDifficultyNotFound(BlockNumber),
    /// when required header related data was not found but was required.
    #[from(ignore)]
    #[display(fmt = "no header found for {_0:?}")]
    HeaderNotFound(BlockHashOrNumber),
    /// The specific transaction is missing.
    #[from(ignore)]
    #[display(fmt = "no transaction found for {_0:?}")]
    TransactionNotFound(TxHashOrNumber),
    /// The specific receipt is missing
    #[from(ignore)]
    #[display(fmt = "no receipt found for {_0:?}")]
    ReceiptNotFound(TxHashOrNumber),
    /// Unable to find the best block.
    #[display(fmt = "best block does not exist")]
    BestBlockNotFound,
    /// Unable to find the finalized block.
    #[display(fmt = "finalized block does not exist")]
    FinalizedBlockNotFound,
    /// Unable to find the safe block.
    #[display(fmt = "safe block does not exist")]
    SafeBlockNotFound,
    /// Mismatch of sender and transaction.
    #[from(ignore)]
    #[display(fmt = "mismatch of sender and transaction id {tx_id}")]
    MismatchOfTransactionAndSenderId {
        /// The transaction ID.
        tx_id: TxNumber,
    },
    /// Block body wrong transaction count.
    #[display(fmt = "stored block indices does not match transaction count")]
    BlockBodyTransactionCount,
    /// Thrown when the cache service task dropped.
    #[display(fmt = "cache service task stopped")]
    CacheServiceUnavailable,
    /// Thrown when we failed to lookup a block for the pending state.
    #[from(ignore)]
    #[display(fmt = "unknown block {_0}")]
    UnknownBlockHash(B256),
    /// Thrown when we were unable to find a state for a block hash.
    #[from(ignore)]
    #[display(fmt = "no state found for block {_0}")]
    StateForHashNotFound(B256),
    /// Unable to find the block number for a given transaction index.
    #[display(fmt = "unable to find the block number for a given transaction index")]
    BlockNumberForTransactionIndexNotFound,
    /// Root mismatch.
    #[from(ignore)]
    #[display(fmt = "merkle trie {_0}")]
    StateRootMismatch(Box<RootMismatch>),
    /// Root mismatch during unwind
    #[display(fmt = "unwind merkle trie {_0}")]
    UnwindStateRootMismatch(Box<RootMismatch>),
    /// State is not available for the given block number because it is pruned.
    #[from(ignore)]
    #[display(fmt = "state at block #{_0} is pruned")]
    StateAtBlockPruned(BlockNumber),
    /// Provider does not support this particular request.
    #[display(fmt = "this provider does not support this request")]
    UnsupportedProvider,
    /// Static File is not found at specified path.
    #[cfg(feature = "std")]
    #[display(fmt = "not able to find {_0} static file at {_1:?}")]
    MissingStaticFilePath(StaticFileSegment, PathBuf),
    /// Static File is not found for requested block.
    #[from(ignore)]
    #[display(fmt = "not able to find {_0} static file for block number {_1}")]
    MissingStaticFileBlock(StaticFileSegment, BlockNumber),
    /// Static File is not found for requested transaction.
    #[from(ignore)]
    #[display(fmt = "unable to find {_0} static file for transaction id {_1}")]
    MissingStaticFileTx(StaticFileSegment, TxNumber),
    /// Static File is finalized and cannot be written to.
    #[from(ignore)]
    #[display(fmt = "unable to write block #{_1} to finalized static file {_0}")]
    FinalizedStaticFile(StaticFileSegment, BlockNumber),
    /// Trying to insert data from an unexpected block number.
    #[from(ignore)]
    #[display(fmt = "trying to append data to {_0} as block #{_1} but expected block #{_2}")]
    UnexpectedStaticFileBlockNumber(StaticFileSegment, BlockNumber, BlockNumber),
    /// Static File Provider was initialized as read-only.
    #[display(fmt = "cannot get a writer on a read-only environment.")]
    ReadOnlyStaticFileAccess,
    /// Error encountered when the block number conversion from U256 to u64 causes an overflow.
    #[from(ignore)]
    #[display(fmt = "failed to convert block number U256 to u64: {_0}")]
    BlockNumberOverflow(U256),
    /// Consistent view error.
    #[display(fmt = "failed to initialize consistent view: {_0}")]
    ConsistentView(Box<ConsistentViewError>),
    /// Storage lock error.
    StorageLockError(crate::lockfile::StorageLockError),
    /// Storage writer error.
    UnifiedStorageWriterError(crate::writer::UnifiedStorageWriterError),
}

#[cfg(feature = "std")]
impl std::error::Error for ProviderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(source) => {
                std::error::Error::source(source)
            },
            Self::Rlp(source) => {
                std::error::Error::source(source)
            },
            Self::StorageLockError(source) => {
                std::error::Error::source(source)
            },
            Self::UnifiedStorageWriterError(source) => {
                std::error::Error::source(source)
            },
            _ => Option::None
        }
    }
}

/// A root mismatch error at a given block height.
#[derive(Clone, Debug, PartialEq, Eq, derive_more::Display)]
#[display(fmt = "root mismatch at #{block_number} ({block_hash}): {root}")]
pub struct RootMismatch {
    /// The target block root diff.
    pub root: GotExpected<B256>,
    /// The target block number.
    pub block_number: BlockNumber,
    /// The target block hash.
    pub block_hash: BlockHash,
}

/// Consistent database view error.
#[derive(Clone, Debug, PartialEq, Eq, Display, From)]
pub enum ConsistentViewError {
    /// Error thrown on attempt to initialize provider while node is still syncing.
    #[display(fmt = "node is syncing. best block: {best_block:?}")]
    Syncing {
        /// Best block diff.
        best_block: GotExpected<BlockNumber>,
    },
    /// Error thrown on inconsistent database view.
    #[display(fmt = "inconsistent database state: {tip:?}")]
    Inconsistent {
        /// The tip diff.
        tip: GotExpected<Option<B256>>,
    },
}

impl From<ConsistentViewError> for ProviderError {
    fn from(value: ConsistentViewError) -> Self {
        Self::ConsistentView(Box::new(value))
    }
}

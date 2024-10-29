use crate::{db::DatabaseError, lockfile::StorageLockError, writer::UnifiedStorageWriterError};
use alloy_eips::BlockHashOrNumber;
use alloy_primitives::{Address, BlockHash, BlockNumber, TxNumber, B256, U256};
use derive_more::Display;
use reth_primitives::{GotExpected, StaticFileSegment, TxHashOrNumber};

#[cfg(feature = "std")]
use std::path::PathBuf;

use alloc::{boxed::Box, string::String};

/// Provider result type.
pub type ProviderResult<Ok> = Result<Ok, ProviderError>;

/// Bundled errors variants thrown by various providers.
#[derive(Clone, Debug, Display, PartialEq, Eq)]
pub enum ProviderError {
    /// Database error.
    Database(DatabaseError),
    /// RLP error.
    Rlp(alloy_rlp::Error),
    /// Filesystem path error.
    #[display("{_0}")]
    FsPathError(String),
    /// Nippy jar error.
    #[display("nippy jar error: {_0}")]
    NippyJar(String),
    /// Trie witness error.
    #[display("trie witness error: {_0}")]
    TrieWitnessError(String),
    /// Error when recovering the sender for a transaction
    #[display("failed to recover sender for transaction")]
    SenderRecoveryError,
    /// The header number was not found for the given block hash.
    #[display("block hash {_0} does not exist in Headers table")]
    BlockHashNotFound(BlockHash),
    /// A block body is missing.
    #[display("block meta not found for block #{_0}")]
    BlockBodyIndicesNotFound(BlockNumber),
    /// The transition ID was found for the given address and storage key, but the changeset was
    /// not found.
    #[display(
        "storage change set for address {address} and key {storage_key} at block #{block_number} does not exist"
    )]
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
    #[display("account change set for address {address} at block #{block_number} does not exist")]
    AccountChangesetNotFound {
        /// Block number found for the address.
        block_number: BlockNumber,
        /// The account address.
        address: Address,
    },
    /// The total difficulty for a block is missing.
    #[display("total difficulty not found for block #{_0}")]
    TotalDifficultyNotFound(BlockNumber),
    /// when required header related data was not found but was required.
    #[display("no header found for {_0:?}")]
    HeaderNotFound(BlockHashOrNumber),
    /// The specific transaction is missing.
    #[display("no transaction found for {_0:?}")]
    TransactionNotFound(TxHashOrNumber),
    /// The specific receipt is missing
    #[display("no receipt found for {_0:?}")]
    ReceiptNotFound(TxHashOrNumber),
    /// Unable to find the best block.
    #[display("best block does not exist")]
    BestBlockNotFound,
    /// Unable to find the finalized block.
    #[display("finalized block does not exist")]
    FinalizedBlockNotFound,
    /// Unable to find the safe block.
    #[display("safe block does not exist")]
    SafeBlockNotFound,
    /// Mismatch of sender and transaction.
    #[display("mismatch of sender and transaction id {tx_id}")]
    MismatchOfTransactionAndSenderId {
        /// The transaction ID.
        tx_id: TxNumber,
    },
    /// Block body wrong transaction count.
    #[display("stored block indices does not match transaction count")]
    BlockBodyTransactionCount,
    /// Thrown when the cache service task dropped.
    #[display("cache service task stopped")]
    CacheServiceUnavailable,
    /// Thrown when we failed to lookup a block for the pending state.
    #[display("unknown block {_0}")]
    UnknownBlockHash(B256),
    /// Thrown when we were unable to find a state for a block hash.
    #[display("no state found for block {_0}")]
    StateForHashNotFound(B256),
    /// Thrown when we were unable to find a state for a block number.
    #[display("no state found for block number {_0}")]
    StateForNumberNotFound(u64),
    /// Unable to find the block number for a given transaction index.
    #[display("unable to find the block number for a given transaction index")]
    BlockNumberForTransactionIndexNotFound,
    /// Root mismatch.
    #[display("merkle trie {_0}")]
    StateRootMismatch(Box<RootMismatch>),
    /// Root mismatch during unwind
    #[display("unwind merkle trie {_0}")]
    UnwindStateRootMismatch(Box<RootMismatch>),
    /// State is not available for the given block number because it is pruned.
    #[display("state at block #{_0} is pruned")]
    StateAtBlockPruned(BlockNumber),
    /// Provider does not support this particular request.
    #[display("this provider does not support this request")]
    UnsupportedProvider,
    /// Static File is not found at specified path.
    #[cfg(feature = "std")]
    #[display("not able to find {_0} static file at {_1:?}")]
    MissingStaticFilePath(StaticFileSegment, PathBuf),
    /// Static File is not found for requested block.
    #[display("not able to find {_0} static file for block number {_1}")]
    MissingStaticFileBlock(StaticFileSegment, BlockNumber),
    /// Static File is not found for requested transaction.
    #[display("unable to find {_0} static file for transaction id {_1}")]
    MissingStaticFileTx(StaticFileSegment, TxNumber),
    /// Static File is finalized and cannot be written to.
    #[display("unable to write block #{_1} to finalized static file {_0}")]
    FinalizedStaticFile(StaticFileSegment, BlockNumber),
    /// Trying to insert data from an unexpected block number.
    #[display("trying to append data to {_0} as block #{_1} but expected block #{_2}")]
    UnexpectedStaticFileBlockNumber(StaticFileSegment, BlockNumber, BlockNumber),
    /// Static File Provider was initialized as read-only.
    #[display("cannot get a writer on a read-only environment.")]
    ReadOnlyStaticFileAccess,
    /// Error encountered when the block number conversion from U256 to u64 causes an overflow.
    #[display("failed to convert block number U256 to u64: {_0}")]
    BlockNumberOverflow(U256),
    /// Consistent view error.
    #[display("failed to initialize consistent view: {_0}")]
    ConsistentView(Box<ConsistentViewError>),
    /// Storage lock error.
    StorageLockError(StorageLockError),
    /// Storage writer error.
    UnifiedStorageWriterError(UnifiedStorageWriterError),
}

impl From<DatabaseError> for ProviderError {
    fn from(error: DatabaseError) -> Self {
        Self::Database(error)
    }
}

impl From<alloy_rlp::Error> for ProviderError {
    fn from(error: alloy_rlp::Error) -> Self {
        Self::Rlp(error)
    }
}

impl From<StorageLockError> for ProviderError {
    fn from(error: StorageLockError) -> Self {
        Self::StorageLockError(error)
    }
}

impl From<UnifiedStorageWriterError> for ProviderError {
    fn from(error: UnifiedStorageWriterError) -> Self {
        Self::UnifiedStorageWriterError(error)
    }
}

impl core::error::Error for ProviderError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Database(source) => core::error::Error::source(source),
            Self::Rlp(source) => core::error::Error::source(source),
            Self::StorageLockError(source) => core::error::Error::source(source),
            Self::UnifiedStorageWriterError(source) => core::error::Error::source(source),
            _ => Option::None,
        }
    }
}

/// A root mismatch error at a given block height.
#[derive(Clone, Debug, PartialEq, Eq, Display)]
#[display("root mismatch at #{block_number} ({block_hash}): {root}")]
pub struct RootMismatch {
    /// The target block root diff.
    pub root: GotExpected<B256>,
    /// The target block number.
    pub block_number: BlockNumber,
    /// The target block hash.
    pub block_hash: BlockHash,
}

/// Consistent database view error.
#[derive(Clone, Debug, PartialEq, Eq, Display)]
pub enum ConsistentViewError {
    /// Error thrown on attempt to initialize provider while node is still syncing.
    #[display("node is syncing. best block: {best_block:?}")]
    Syncing {
        /// Best block diff.
        best_block: GotExpected<BlockNumber>,
    },
    /// Error thrown on inconsistent database view.
    #[display("inconsistent database state: {tip:?}")]
    Inconsistent {
        /// The tip diff.
        tip: GotExpected<Option<B256>>,
    },
}

impl From<ConsistentViewError> for ProviderError {
    fn from(error: ConsistentViewError) -> Self {
        Self::ConsistentView(Box::new(error))
    }
}

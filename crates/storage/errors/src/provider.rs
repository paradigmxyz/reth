use crate::{any::AnyError, db::DatabaseError, writer::UnifiedStorageWriterError};
use alloc::{boxed::Box, string::String};
use alloy_eips::{BlockHashOrNumber, HashOrNumber};
use alloy_primitives::{Address, BlockHash, BlockNumber, TxNumber, B256};
use derive_more::Display;
use reth_primitives_traits::{transaction::signed::RecoveryError, GotExpected};
use reth_prune_types::PruneSegmentError;
use reth_static_file_types::StaticFileSegment;
use revm_database_interface::DBErrorMarker;

/// Provider result type.
pub type ProviderResult<Ok> = Result<Ok, ProviderError>;

/// Bundled errors variants thrown by various providers.
#[derive(Clone, Debug, thiserror::Error)]
pub enum ProviderError {
    /// Database error.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// Pruning error.
    #[error(transparent)]
    Pruning(#[from] PruneSegmentError),
    /// RLP error.
    #[error("{_0}")]
    Rlp(alloy_rlp::Error),
    /// Trie witness error.
    #[error("trie witness error: {_0}")]
    TrieWitnessError(String),
    /// Error when recovering the sender for a transaction
    #[error("failed to recover sender for transaction")]
    SenderRecoveryError,
    /// The header number was not found for the given block hash.
    #[error("block hash {_0} does not exist in Headers table")]
    BlockHashNotFound(BlockHash),
    /// A block body is missing.
    #[error("block meta not found for block #{_0}")]
    BlockBodyIndicesNotFound(BlockNumber),
    /// The transition ID was found for the given address and storage key, but the changeset was
    /// not found.
    #[error(
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
    #[error("account change set for address {address} at block #{block_number} does not exist")]
    AccountChangesetNotFound {
        /// Block number found for the address.
        block_number: BlockNumber,
        /// The account address.
        address: Address,
    },
    /// The total difficulty for a block is missing.
    #[error("total difficulty not found for block #{_0}")]
    TotalDifficultyNotFound(BlockNumber),
    /// When required header related data was not found but was required.
    #[error("no header found for {_0:?}")]
    HeaderNotFound(BlockHashOrNumber),
    /// The specific transaction identified by hash or id is missing.
    #[error("no transaction found for {_0:?}")]
    TransactionNotFound(HashOrNumber),
    /// The specific receipt for a transaction identified by hash or id is missing
    #[error("no receipt found for {_0:?}")]
    ReceiptNotFound(HashOrNumber),
    /// Unable to find the best block.
    #[error("best block does not exist")]
    BestBlockNotFound,
    /// Unable to find the finalized block.
    #[error("finalized block does not exist")]
    FinalizedBlockNotFound,
    /// Unable to find the safe block.
    #[error("safe block does not exist")]
    SafeBlockNotFound,
    /// Thrown when we failed to lookup a block for the pending state.
    #[error("unknown block {_0}")]
    UnknownBlockHash(B256),
    /// Thrown when we were unable to find a state for a block hash.
    #[error("no state found for block {_0}")]
    StateForHashNotFound(B256),
    /// Thrown when we were unable to find a state for a block number.
    #[error("no state found for block number {_0}")]
    StateForNumberNotFound(u64),
    /// Unable to find the block number for a given transaction index.
    #[error("unable to find the block number for a given transaction index")]
    BlockNumberForTransactionIndexNotFound,
    /// Root mismatch.
    #[error("merkle trie {_0}")]
    StateRootMismatch(Box<RootMismatch>),
    /// Root mismatch during unwind
    #[error("unwind merkle trie {_0}")]
    UnwindStateRootMismatch(Box<RootMismatch>),
    /// State is not available for the given block number because it is pruned.
    #[error("state at block #{_0} is pruned")]
    StateAtBlockPruned(BlockNumber),
    /// Provider does not support this particular request.
    #[error("this provider does not support this request")]
    UnsupportedProvider,
    /// Static File is not found at specified path.
    #[cfg(feature = "std")]
    #[error("not able to find {_0} static file at {_1:?}")]
    MissingStaticFilePath(StaticFileSegment, std::path::PathBuf),
    /// Static File is not found for requested block.
    #[error("not able to find {_0} static file for block number {_1}")]
    MissingStaticFileBlock(StaticFileSegment, BlockNumber),
    /// Static File is not found for requested transaction.
    #[error("unable to find {_0} static file for transaction id {_1}")]
    MissingStaticFileTx(StaticFileSegment, TxNumber),
    /// Static File is finalized and cannot be written to.
    #[error("unable to write block #{_1} to finalized static file {_0}")]
    FinalizedStaticFile(StaticFileSegment, BlockNumber),
    /// Trying to insert data from an unexpected block number.
    #[error("trying to append data to {_0} as block #{_1} but expected block #{_2}")]
    UnexpectedStaticFileBlockNumber(StaticFileSegment, BlockNumber, BlockNumber),
    /// Trying to insert data from an unexpected block number.
    #[error("trying to append row to {_0} at index #{_1} but expected index #{_2}")]
    UnexpectedStaticFileTxNumber(StaticFileSegment, TxNumber, TxNumber),
    /// Static File Provider was initialized as read-only.
    #[error("cannot get a writer on a read-only environment.")]
    ReadOnlyStaticFileAccess,
    /// Consistent view error.
    #[error("failed to initialize consistent view: {_0}")]
    ConsistentView(Box<ConsistentViewError>),
    /// Storage writer error.
    #[error(transparent)]
    UnifiedStorageWriterError(#[from] UnifiedStorageWriterError),
    /// Received invalid output from configured storage implementation.
    #[error("received invalid output from storage")]
    InvalidStorageOutput,
    /// Missing trie updates.
    #[error("missing trie updates for block {0}")]
    MissingTrieUpdates(B256),
    /// Any other error type wrapped into a cloneable [`AnyError`].
    #[error(transparent)]
    Other(#[from] AnyError),
}

impl ProviderError {
    /// Creates a new [`ProviderError::Other`] variant by wrapping the given error into an
    /// [`AnyError`]
    pub fn other<E>(error: E) -> Self
    where
        E: core::error::Error + Send + Sync + 'static,
    {
        Self::Other(AnyError::new(error))
    }

    /// Returns the arbitrary error if it is [`ProviderError::Other`]
    pub fn as_other(&self) -> Option<&(dyn core::error::Error + Send + Sync + 'static)> {
        match self {
            Self::Other(err) => Some(err.as_error()),
            _ => None,
        }
    }

    /// Returns a reference to the [`ProviderError::Other`] value if this type is a
    /// [`ProviderError::Other`] and the [`AnyError`] wraps an error of that type. Returns None
    /// otherwise.
    pub fn downcast_other_ref<T: core::error::Error + 'static>(&self) -> Option<&T> {
        let other = self.as_other()?;
        other.downcast_ref()
    }

    /// Returns true if the this type is a [`ProviderError::Other`] of that error
    /// type. Returns false otherwise.
    pub fn is_other<T: core::error::Error + 'static>(&self) -> bool {
        self.as_other().map(|err| err.is::<T>()).unwrap_or(false)
    }
}

impl DBErrorMarker for ProviderError {}

impl From<alloy_rlp::Error> for ProviderError {
    fn from(error: alloy_rlp::Error) -> Self {
        Self::Rlp(error)
    }
}

impl From<RecoveryError> for ProviderError {
    fn from(_: RecoveryError) -> Self {
        Self::SenderRecoveryError
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

/// A Static File Write Error.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct StaticFileWriterError {
    /// The error message.
    pub message: String,
}

impl StaticFileWriterError {
    /// Creates a new [`StaticFileWriterError`] with the given message.
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
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
    /// Error thrown when the database does not contain a block from the previous database view.
    #[display("database view no longer contains block: {block:?}")]
    Reorged {
        /// The previous block
        block: B256,
    },
}

impl From<ConsistentViewError> for ProviderError {
    fn from(error: ConsistentViewError) -> Self {
        Self::ConsistentView(Box::new(error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(thiserror::Error, Debug)]
    #[error("E")]
    struct E;

    #[test]
    fn other_err() {
        let err = ProviderError::other(E);
        assert!(err.is_other::<E>());
        assert!(err.downcast_other_ref::<E>().is_some());
    }
}

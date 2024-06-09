use crate::{db::DatabaseError, lockfile::StorageLockError};
use core::fmt::{Display, Formatter, Result};
use reth_fs_util::FsPathError;
use reth_primitives::{
    Address, BlockHash, BlockHashOrNumber, BlockNumber, GotExpected, StaticFileSegment,
    TxHashOrNumber, TxNumber, B256, U256,
};
use std::path::PathBuf;

/// Provider result type.
pub type ProviderResult<Ok> = std::result::Result<Ok, ProviderError>;

/// Bundled errors variants thrown by various providers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderError {
    /// Database error.
    Database(DatabaseError),
    /// Filesystem path error.
    FsPathError(String),
    /// Nippy jar error.
    NippyJar(String),
    /// Error when recovering the sender for a transaction
    SenderRecoveryError,
    /// The header number was not found for the given block hash.
    BlockHashNotFound(BlockHash),
    /// A block body is missing.
    BlockBodyIndicesNotFound(BlockNumber),
    /// The transition ID was found for the given address and storage key, but the changeset was
    /// not found.
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
    AccountChangesetNotFound {
        /// Block number found for the address.
        block_number: BlockNumber,
        /// The account address.
        address: Address,
    },
    /// The total difficulty for a block is missing.
    TotalDifficultyNotFound(BlockNumber),
    /// when required header related data was not found but was required.
    HeaderNotFound(BlockHashOrNumber),
    /// The specific transaction is missing.
    TransactionNotFound(TxHashOrNumber),
    /// The specific receipt is missing
    ReceiptNotFound(TxHashOrNumber),
    /// Unable to find the best block.
    BestBlockNotFound,
    /// Unable to find the finalized block.
    FinalizedBlockNotFound,
    /// Unable to find the safe block.
    SafeBlockNotFound,
    /// Mismatch of sender and transaction.
    MismatchOfTransactionAndSenderId {
        /// The transaction ID.
        tx_id: TxNumber,
    },
    /// Block body wrong transaction count.
    BlockBodyTransactionCount,
    /// Thrown when the cache service task dropped.
    CacheServiceUnavailable,
    /// Thrown when we failed to lookup a block for the pending state.
    UnknownBlockHash(B256),
    /// Thrown when we were unable to find a state for a block hash.
    StateForHashNotFound(B256),
    /// Unable to compute state root on top of historical block.
    StateRootNotAvailableForHistoricalBlock,
    /// Unable to find the block number for a given transaction index.
    BlockNumberForTransactionIndexNotFound,
    /// Root mismatch.
    StateRootMismatch(Box<RootMismatch>),
    /// Root mismatch during unwind
    UnwindStateRootMismatch(Box<RootMismatch>),
    /// State is not available for the given block number because it is pruned.
    StateAtBlockPruned(BlockNumber),
    /// Provider does not support this particular request.
    UnsupportedProvider,
    /// Static File is not found at specified path.
    MissingStaticFilePath(StaticFileSegment, PathBuf),
    /// Static File is not found for requested block.
    MissingStaticFileBlock(StaticFileSegment, BlockNumber),
    /// Static File is not found for requested transaction.
    MissingStaticFileTx(StaticFileSegment, TxNumber),
    /// Static File is finalized and cannot be written to.
    FinalizedStaticFile(StaticFileSegment, BlockNumber),
    /// Trying to insert data from an unexpected block number.
    UnexpectedStaticFileBlockNumber(StaticFileSegment, BlockNumber, BlockNumber),
    /// Static File Provider was initialized as read-only.
    ReadOnlyStaticFileAccess,
    /// Error encountered when the block number conversion from U256 to u64 causes an overflow.
    BlockNumberOverflow(U256),
    /// Consistent view error.
    ConsistentView(Box<ConsistentViewError>),
    /// Storage lock error.
    StorageLockError(crate::lockfile::StorageLockError),
}

#[cfg(feature = "std")]
impl std::error::Error for ProviderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(ref e) => Some(e),
            Self::StorageLockError(ref e) => Some(e),
            _ => None,
        }
    }
}

impl Display for ProviderError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            Self::Database(db_error) => write!(f, "Database error: {}", db_error),
            Self::FsPathError(path_error) => write!(f, "Filesystem path error: {}", path_error),
            Self::NippyJar(error) => write!(f, "Nippy jar error: {}", error),
            Self::SenderRecoveryError => write!(f, "Failed to recover sender for transaction"),
            Self::BlockHashNotFound(block_hash) => {
                write!(f, "Block hash {} does not exist in Headers table", block_hash)
            }
            Self::BlockBodyIndicesNotFound(block_number) => {
                write!(f, "Block meta not found for block #{}", block_number)
            }
            Self::StorageChangesetNotFound { block_number, address, storage_key } => write!(
                f,
                "Storage change set for address {} and key {} at block #{} does not exist",
                address, storage_key, block_number
            ),
            Self::AccountChangesetNotFound { block_number, address } => write!(
                f,
                "Account change set for address {} at block #{} does not exist",
                address, block_number
            ),
            Self::TotalDifficultyNotFound(block_number) => {
                write!(f, "Total difficulty not found for block #{}", block_number)
            }
            Self::HeaderNotFound(header) => write!(f, "No header found for {:?}", header),
            Self::TransactionNotFound(tx) => write!(f, "No transaction found for {:?}", tx),
            Self::ReceiptNotFound(tx) => write!(f, "No receipt found for {:?}", tx),
            Self::BestBlockNotFound => write!(f, "Best block does not exist"),
            Self::FinalizedBlockNotFound => write!(f, "Finalized block does not exist"),
            Self::SafeBlockNotFound => write!(f, "Safe block does not exist"),
            Self::MismatchOfTransactionAndSenderId { tx_id } => {
                write!(f, "Mismatch of sender and transaction id {}", tx_id)
            }
            Self::BlockBodyTransactionCount => {
                write!(f, "Stored block indices do not match transaction count")
            }
            Self::CacheServiceUnavailable => write!(f, "Cache service task stopped"),
            Self::UnknownBlockHash(block_hash) => write!(f, "Unknown block {}", block_hash),
            Self::StateForHashNotFound(block_hash) => {
                write!(f, "No state found for block {}", block_hash)
            }
            Self::StateRootNotAvailableForHistoricalBlock => {
                write!(f, "Unable to compute state root on top of historical block")
            }
            Self::BlockNumberForTransactionIndexNotFound => {
                write!(f, "Unable to find the block number for a given transaction index")
            }
            Self::StateRootMismatch(root_mismatch) => {
                write!(f, "Merkle trie root mismatch: {}", root_mismatch)
            }
            Self::UnwindStateRootMismatch(root_mismatch) => {
                write!(f, "Unwind merkle trie root mismatch: {}", root_mismatch)
            }
            Self::StateAtBlockPruned(block_number) => {
                write!(f, "State at block #{} is pruned", block_number)
            }
            Self::UnsupportedProvider => write!(f, "This provider does not support this request"),
            Self::MissingStaticFilePath(segment, path) => {
                write!(f, "Not able to find {} static file at {}", segment, path.display())
            }
            Self::MissingStaticFileBlock(segment, block_number) => write!(
                f,
                "Not able to find {} static file for block number {}",
                segment, block_number
            ),
            Self::MissingStaticFileTx(segment, tx_number) => {
                write!(f, "Unable to find {} static file for transaction id {}", segment, tx_number)
            }
            Self::FinalizedStaticFile(segment, block_number) => write!(
                f,
                "Unable to write block #{} to finalized static file {}",
                block_number, segment
            ),
            Self::UnexpectedStaticFileBlockNumber(segment, expected, actual) => write!(
                f,
                "Trying to append data to {} as block #{} but expected block #{}",
                segment, actual, expected
            ),
            Self::ReadOnlyStaticFileAccess => {
                write!(f, "Cannot get a writer on a read-only environment")
            }
            Self::BlockNumberOverflow(block_number) => {
                write!(f, "Failed to convert block number U256 to u64: {}", block_number)
            }
            Self::ConsistentView(error) => {
                write!(f, "Failed to initialize consistent view: {}", error)
            }
            Self::StorageLockError(lock_error) => write!(f, "Storage lock error: {}", lock_error),
        }
    }
}

impl From<StorageLockError> for ProviderError {
    fn from(err: StorageLockError) -> Self {
        Self::StorageLockError(err)
    }
}

impl From<DatabaseError> for ProviderError {
    fn from(err: DatabaseError) -> Self {
        Self::Database(err) // Assuming `Database` is a variant of `ProviderError`
    }
}

impl From<FsPathError> for ProviderError {
    fn from(err: FsPathError) -> Self {
        Self::FsPathError(err.to_string())
    }
}

/// A root mismatch error at a given block height.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootMismatch {
    /// The target block root diff.
    pub root: GotExpected<B256>,
    /// The target block number.
    pub block_number: BlockNumber,
    /// The target block hash.
    pub block_hash: BlockHash,
}

#[cfg(feature = "std")]
impl std::error::Error for RootMismatch {}

impl Display for RootMismatch {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        write!(f, "root mismatch at #{} ({}): {}", self.block_number, self.block_hash, self.root)
    }
}

/// Consistent database view error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConsistentViewError {
    /// Error thrown on attempt to initialize provider while node is still syncing.
    Syncing {
        /// Best block diff.
        best_block: GotExpected<BlockNumber>,
    },
    /// Error thrown on inconsistent database view.
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

#[cfg(feature = "std")]
impl std::error::Error for ConsistentViewError {}

impl Display for ConsistentViewError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            Self::Syncing { best_block } => {
                f.write_fmt(format_args!("node is syncing. best block: {0:?}", best_block,))
            }
            Self::Inconsistent { tip } => {
                f.write_fmt(format_args!("inconsistent database state: {0:?}", tip))
            }
        }
    }
}

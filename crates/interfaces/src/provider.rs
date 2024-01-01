use reth_primitives::{
    Address, BlockHash, BlockHashOrNumber, BlockNumber, GotExpected, SnapshotSegment,
    TxHashOrNumber, TxNumber, B256, U256,
};
use std::path::PathBuf;
use thiserror::Error;

/// Provider result type.
pub type ProviderResult<Ok> = Result<Ok, ProviderError>;

/// Bundled errors variants thrown by various providers.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ProviderError {
    /// Database error.
    #[error(transparent)]
    Database(#[from] crate::db::DatabaseError),
    /// Filesystem path error.
    #[error("{0}")]
    FsPathError(String),
    /// Nippy jar error.
    #[error("nippy jar error: {0}")]
    NippyJar(String),
    /// Error when recovering the sender for a transaction
    #[error("failed to recover sender for transaction")]
    SenderRecoveryError,
    /// Inconsistent header gap.
    #[error("inconsistent header gap in the database")]
    InconsistentHeaderGap,
    /// The header number was not found for the given block hash.
    #[error("block hash {0} does not exist in Headers table")]
    BlockHashNotFound(BlockHash),
    /// A block body is missing.
    #[error("block meta not found for block #{0}")]
    BlockBodyIndicesNotFound(BlockNumber),
    /// The transition ID was found for the given address and storage key, but the changeset was
    /// not found.
    #[error("storage change set for address {address} and key {storage_key} at block #{block_number} does not exist")]
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
    #[error("total difficulty not found for block #{0}")]
    TotalDifficultyNotFound(BlockNumber),
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
    #[error("merkle trie {0}")]
    StateRootMismatch(Box<RootMismatch>),
    /// Root mismatch during unwind
    #[error("unwind merkle trie {0}")]
    UnwindStateRootMismatch(Box<RootMismatch>),
    /// State is not available for the given block number because it is pruned.
    #[error("state at block #{0} is pruned")]
    StateAtBlockPruned(BlockNumber),
    /// Provider does not support this particular request.
    #[error("this provider does not support this request")]
    UnsupportedProvider,
    /// Snapshot file is not found at specified path.
    #[error("not able to find {0} snapshot file at {1}")]
    MissingSnapshotPath(SnapshotSegment, PathBuf),
    /// Snapshot file is not found for requested block.
    #[error("not able to find {0} snapshot file for block number {1}")]
    MissingSnapshotBlock(SnapshotSegment, BlockNumber),
    /// Snapshot file is not found for requested transaction.
    #[error("not able to find {0} snapshot file for transaction id {1}")]
    MissingSnapshotTx(SnapshotSegment, TxNumber),
    /// Error encountered when the block number conversion from U256 to u64 causes an overflow.
    #[error("failed to convert block number U256 to u64: {0}")]
    BlockNumberOverflow(U256),
}

impl From<reth_nippy_jar::NippyJarError> for ProviderError {
    fn from(err: reth_nippy_jar::NippyJarError) -> Self {
        ProviderError::NippyJar(err.to_string())
    }
}

impl From<reth_primitives::fs::FsPathError> for ProviderError {
    fn from(err: reth_primitives::fs::FsPathError) -> Self {
        ProviderError::FsPathError(err.to_string())
    }
}

/// A root mismatch error at a given block height.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("root mismatch at #{block_number} ({block_hash}): {root}")]
pub struct RootMismatch {
    /// The target block root diff.
    pub root: GotExpected<B256>,
    /// The target block number.
    pub block_number: BlockNumber,
    /// The target block hash.
    pub block_hash: BlockHash,
}

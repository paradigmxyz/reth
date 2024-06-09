use core::fmt::{Display, Formatter, Result};
use reth_primitives::{
    Address, BlockHash, BlockHashOrNumber, BlockNumber, GotExpected, StaticFileSegment,
    TxHashOrNumber, TxNumber, B256, U256,
};
use std::path::PathBuf;

#[cfg(feature = "std")]
use std::error::Error;

/// Provider result type.
pub type ProviderResult<Ok> = std::result::Result<Ok, ProviderError>;

/// Bundled errors variants thrown by various providers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderError {
    /// Database error.
    Database(crate::db::DatabaseError),
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

impl Error for ProviderError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        use thiserror::__private::AsDynError as _;
        #[allow(deprecated)]
        match self {
            ProviderError::Database { 0: transparent } => {
                std::error::Error::source(transparent.as_dyn_error())
            }
            ProviderError::FsPathError { .. } => ::core::option::Option::None,
            ProviderError::NippyJar { .. } => ::core::option::Option::None,
            ProviderError::SenderRecoveryError { .. } => ::core::option::Option::None,
            ProviderError::BlockHashNotFound { .. } => ::core::option::Option::None,
            ProviderError::BlockBodyIndicesNotFound { .. } => ::core::option::Option::None,
            ProviderError::StorageChangesetNotFound { .. } => ::core::option::Option::None,
            ProviderError::AccountChangesetNotFound { .. } => ::core::option::Option::None,
            ProviderError::TotalDifficultyNotFound { .. } => ::core::option::Option::None,
            ProviderError::HeaderNotFound { .. } => ::core::option::Option::None,
            ProviderError::TransactionNotFound { .. } => ::core::option::Option::None,
            ProviderError::ReceiptNotFound { .. } => ::core::option::Option::None,
            ProviderError::BestBlockNotFound { .. } => ::core::option::Option::None,
            ProviderError::FinalizedBlockNotFound { .. } => ::core::option::Option::None,
            ProviderError::SafeBlockNotFound { .. } => ::core::option::Option::None,
            ProviderError::MismatchOfTransactionAndSenderId { .. } => ::core::option::Option::None,
            ProviderError::BlockBodyTransactionCount { .. } => ::core::option::Option::None,
            ProviderError::CacheServiceUnavailable { .. } => ::core::option::Option::None,
            ProviderError::UnknownBlockHash { .. } => ::core::option::Option::None,
            ProviderError::StateForHashNotFound { .. } => ::core::option::Option::None,
            ProviderError::StateRootNotAvailableForHistoricalBlock { .. } => {
                ::core::option::Option::None
            }
            ProviderError::BlockNumberForTransactionIndexNotFound { .. } => {
                ::core::option::Option::None
            }
            ProviderError::StateRootMismatch { .. } => ::core::option::Option::None,
            ProviderError::UnwindStateRootMismatch { .. } => ::core::option::Option::None,
            ProviderError::StateAtBlockPruned { .. } => ::core::option::Option::None,
            ProviderError::UnsupportedProvider { .. } => ::core::option::Option::None,
            ProviderError::MissingStaticFilePath { .. } => ::core::option::Option::None,
            ProviderError::MissingStaticFileBlock { .. } => ::core::option::Option::None,
            ProviderError::MissingStaticFileTx { .. } => ::core::option::Option::None,
            ProviderError::FinalizedStaticFile { .. } => ::core::option::Option::None,
            ProviderError::UnexpectedStaticFileBlockNumber { .. } => ::core::option::Option::None,
            ProviderError::ReadOnlyStaticFileAccess { .. } => ::core::option::Option::None,
            ProviderError::BlockNumberOverflow { .. } => ::core::option::Option::None,
            ProviderError::ConsistentView { .. } => ::core::option::Option::None,
            ProviderError::StorageLockError { 0: transparent } => {
                std::error::Error::source(transparent.as_dyn_error())
            }
        }
    }
}

impl Display for ProviderError {
    fn fmt(&self, __formatter: &mut Formatter<'_>) -> Result {
        use thiserror::__private::AsDisplay as _;
        #[allow(unused_variables, deprecated, clippy::used_underscore_binding)]
        match self {
            ProviderError::Database(_0) => Display::fmt(_0, __formatter),
            ProviderError::FsPathError(_0) => {
                __formatter.write_fmt(format_args!("{0}", _0.as_display()))
            }
            ProviderError::NippyJar(_0) => {
                __formatter.write_fmt(format_args!("nippy jar error: {0}", _0.as_display()))
            }
            ProviderError::SenderRecoveryError {} => {
                __formatter.write_str("failed to recover sender for transaction")
            }
            ProviderError::BlockHashNotFound(_0) => __formatter.write_fmt(format_args!(
                "block hash {0} does not exist in Headers table",
                _0.as_display(),
            )),
            ProviderError::BlockBodyIndicesNotFound(_0) => __formatter
                .write_fmt(format_args!("block meta not found for block #{0}", _0.as_display(),)),
            ProviderError::StorageChangesetNotFound { block_number, address, storage_key } => {
                __formatter.write_fmt(format_args!(
                    "storage change set for address {0} and key {1} at block #{2} does not exist",
                    address.as_display(),
                    storage_key.as_display(),
                    block_number.as_display(),
                ))
            }
            ProviderError::AccountChangesetNotFound { block_number, address } => __formatter
                .write_fmt(format_args!(
                    "account change set for address {0} at block #{1} does not exist",
                    address.as_display(),
                    block_number.as_display(),
                )),
            ProviderError::TotalDifficultyNotFound(_0) => __formatter.write_fmt(format_args!(
                "total difficulty not found for block #{0}",
                _0.as_display(),
            )),
            ProviderError::HeaderNotFound(_0) => {
                __formatter.write_fmt(format_args!("no header found for {0:?}", _0))
            }
            ProviderError::TransactionNotFound(_0) => {
                __formatter.write_fmt(format_args!("no transaction found for {0:?}", _0))
            }
            ProviderError::ReceiptNotFound(_0) => {
                __formatter.write_fmt(format_args!("no receipt found for {0:?}", _0))
            }
            ProviderError::BestBlockNotFound {} => {
                __formatter.write_str("best block does not exist")
            }
            ProviderError::FinalizedBlockNotFound {} => {
                __formatter.write_str("finalized block does not exist")
            }
            ProviderError::SafeBlockNotFound {} => {
                __formatter.write_str("safe block does not exist")
            }
            ProviderError::MismatchOfTransactionAndSenderId { tx_id } => __formatter.write_fmt(
                format_args!("mismatch of sender and transaction id {0}", tx_id.as_display(),),
            ),
            ProviderError::BlockBodyTransactionCount {} => {
                __formatter.write_str("stored block indices does not match transaction count")
            }
            ProviderError::CacheServiceUnavailable {} => {
                __formatter.write_str("cache service task stopped")
            }
            ProviderError::UnknownBlockHash(_0) => {
                __formatter.write_fmt(format_args!("unknown block {0}", _0.as_display()))
            }
            ProviderError::StateForHashNotFound(_0) => {
                __formatter.write_fmt(format_args!("no state found for block {0}", _0.as_display()))
            }
            ProviderError::StateRootNotAvailableForHistoricalBlock {} => {
                __formatter.write_str("unable to compute state root on top of historical block")
            }
            ProviderError::BlockNumberForTransactionIndexNotFound {} => __formatter
                .write_str("unable to find the block number for a given transaction index"),
            ProviderError::StateRootMismatch(_0) => {
                __formatter.write_fmt(format_args!("merkle trie {0}", _0.as_display()))
            }
            ProviderError::UnwindStateRootMismatch(_0) => {
                __formatter.write_fmt(format_args!("unwind merkle trie {0}", _0.as_display()))
            }
            ProviderError::StateAtBlockPruned(_0) => __formatter
                .write_fmt(format_args!("state at block #{0} is pruned", _0.as_display(),)),
            ProviderError::UnsupportedProvider {} => {
                __formatter.write_str("this provider does not support this request")
            }
            ProviderError::MissingStaticFilePath(_0, _1) => __formatter.write_fmt(format_args!(
                "not able to find {0} static file at {1}",
                _0.as_display(),
                _1.as_display(),
            )),
            ProviderError::MissingStaticFileBlock(_0, _1) => __formatter.write_fmt(format_args!(
                "not able to find {0} static file for block number {1}",
                _0.as_display(),
                _1.as_display(),
            )),
            ProviderError::MissingStaticFileTx(_0, _1) => __formatter.write_fmt(format_args!(
                "unable to find {0} static file for transaction id {1}",
                _0.as_display(),
                _1.as_display(),
            )),
            ProviderError::FinalizedStaticFile(_0, _1) => __formatter.write_fmt(format_args!(
                "unable to write block #{0} to finalized static file {1}",
                _1.as_display(),
                _0.as_display(),
            )),
            ProviderError::UnexpectedStaticFileBlockNumber(_0, _1, _2) => {
                __formatter.write_fmt(format_args!(
                    "trying to append data to {0} as block #{1} but expected block #{2}",
                    _0.as_display(),
                    _1.as_display(),
                    _2.as_display(),
                ))
            }
            ProviderError::ReadOnlyStaticFileAccess {} => {
                __formatter.write_str("cannot get a writer on a read-only environment.")
            }
            ProviderError::BlockNumberOverflow(_0) => __formatter.write_fmt(format_args!(
                "failed to convert block number U256 to u64: {0}",
                _0.as_display(),
            )),
            ProviderError::ConsistentView(_0) => __formatter.write_fmt(format_args!(
                "failed to initialize consistent view: {0}",
                _0.as_display(),
            )),
            ProviderError::StorageLockError(_0) => ::core::fmt::Display::fmt(_0, __formatter),
        }
    }
}

impl From<reth_fs_util::FsPathError> for ProviderError {
    fn from(err: reth_fs_util::FsPathError) -> Self {
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
    #[allow(clippy::used_underscore_binding)]
    fn fmt(&self, __formatter: &mut Formatter<'_>) -> Result {
        use thiserror::__private::AsDisplay as _;
        #[allow(unused_variables, deprecated)]
        let Self { root, block_number, block_hash } = self;
        __formatter.write_fmt(format_args!(
            "root mismatch at #{0} ({1}): {2}",
            block_number.as_display(),
            block_hash.as_display(),
            root.as_display(),
        ))
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
impl Error for ConsistentViewError {}

impl Display for ConsistentViewError {
    fn fmt(&self, __formatter: &mut Formatter<'_>) -> Result {
        match self {
            ConsistentViewError::Syncing { best_block } => __formatter
                .write_fmt(format_args!("node is syncing. best block: {0:?}", best_block,)),
            ConsistentViewError::Inconsistent { tip } => {
                __formatter.write_fmt(format_args!("inconsistent database state: {0:?}", tip))
            }
        }
    }
}

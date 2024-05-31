//! Commonly used error types used when doing block execution.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use reth_consensus::ConsensusError;
use reth_primitives::{revm_primitives::EVMError, BlockNumHash, PruneSegmentError, B256};
use reth_storage_errors::provider::ProviderError;
use std::fmt::Display;
use thiserror::Error;

pub mod trie;
pub use trie::{StateRootError, StorageRootError};

/// Transaction validation errors
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum BlockValidationError {
    /// EVM error with transaction hash and message
    #[error("EVM reported invalid transaction ({hash}): {error}")]
    EVM {
        /// The hash of the transaction
        hash: B256,
        /// The EVM error.
        #[source]
        error: Box<EVMError<ProviderError>>,
    },
    /// Error when recovering the sender for a transaction
    #[error("failed to recover sender for transaction")]
    SenderRecoveryError,
    /// Error when incrementing balance in post execution
    #[error("incrementing balance in post execution failed")]
    IncrementBalanceFailed,
    /// Error when the state root does not match the expected value.
    #[error(transparent)]
    StateRoot(#[from] StateRootError),
    /// Error when transaction gas limit exceeds available block gas
    #[error("transaction gas limit {transaction_gas_limit} is more than blocks available gas {block_available_gas}")]
    TransactionGasLimitMoreThanAvailableBlockGas {
        /// The transaction's gas limit
        transaction_gas_limit: u64,
        /// The available block gas
        block_available_gas: u64,
    },
    /// Error for pre-merge block
    #[error("block {hash} is pre merge")]
    BlockPreMerge {
        /// The hash of the block
        hash: B256,
    },
    /// Error for missing total difficulty
    #[error("missing total difficulty for block {hash}")]
    MissingTotalDifficulty {
        /// The hash of the block
        hash: B256,
    },
    /// Error for EIP-4788 when parent beacon block root is missing
    #[error("EIP-4788 parent beacon block root missing for active Cancun block")]
    MissingParentBeaconBlockRoot,
    /// Error for Cancun genesis block when parent beacon block root is not zero
    #[error("the parent beacon block root is not zero for Cancun genesis block: {parent_beacon_block_root}")]
    CancunGenesisParentBeaconBlockRootNotZero {
        /// The beacon block root
        parent_beacon_block_root: B256,
    },
    /// EVM error during [EIP-4788] beacon root contract call.
    ///
    /// [EIP-4788]: https://eips.ethereum.org/EIPS/eip-4788
    #[error("failed to apply beacon root contract call at {parent_beacon_block_root}: {message}")]
    BeaconRootContractCall {
        /// The beacon block root
        parent_beacon_block_root: Box<B256>,
        /// The error message.
        message: String,
    },
    /// Provider error during the [EIP-2935] block hash account loading.
    ///
    /// [EIP-2935]: https://eips.ethereum.org/EIPS/eip-2935
    #[error(transparent)]
    BlockHashAccountLoadingFailed(#[from] ProviderError),
    /// EVM error during withdrawal requests contract call [EIP-7002]
    ///
    /// [EIP-7002]: https://eips.ethereum.org/EIPS/eip-7002
    #[error("failed to apply withdrawal requests contract call: {message}")]
    WithdrawalRequestsContractCall {
        /// The error message.
        message: String,
    },
    /// Error when decoding deposit requests from receipts [EIP-6110]
    ///
    /// [EIP-6110]: https://eips.ethereum.org/EIPS/eip-6110
    #[error("failed to decode deposit requests from receipts: {0}")]
    DepositRequestDecode(String),
}

/// BlockExecutor Errors
#[derive(Error, Debug)]
pub enum BlockExecutionError {
    /// Validation error, transparently wrapping `BlockValidationError`
    #[error(transparent)]
    Validation(#[from] BlockValidationError),
    /// Pruning error, transparently wrapping `PruneSegmentError`
    #[error(transparent)]
    Pruning(#[from] PruneSegmentError),
    /// Consensus error, transparently wrapping `ConsensusError`
    #[error(transparent)]
    Consensus(#[from] ConsensusError),
    /// Transaction error on revert with inner details
    #[error("transaction error on revert: {inner}")]
    CanonicalRevert {
        /// The inner error message
        inner: String,
    },
    /// Transaction error on commit with inner details
    #[error("transaction error on commit: {inner}")]
    CanonicalCommit {
        /// The inner error message
        inner: String,
    },
    /// Error when appending chain on fork is not possible
    #[error(
        "appending chain on fork (other_chain_fork:?) is not possible as the tip is {chain_tip:?}"
    )]
    AppendChainDoesntConnect {
        /// The tip of the current chain
        chain_tip: Box<BlockNumHash>,
        /// The fork on the other chain
        other_chain_fork: Box<BlockNumHash>,
    },
    /// Error when fetching latest block state.
    #[error(transparent)]
    LatestBlock(#[from] ProviderError),
    /// Arbitrary Block Executor Errors
    #[error(transparent)]
    Other(Box<dyn std::error::Error + Send + Sync>),
}

impl BlockExecutionError {
    /// Create a new `BlockExecutionError::Other` variant.
    pub fn other<E>(error: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Other(Box::new(error))
    }

    /// Create a new [BlockExecutionError::Other] from a given message.
    pub fn msg(msg: impl Display) -> Self {
        Self::Other(msg.to_string().into())
    }

    /// Returns the inner `BlockValidationError` if the error is a validation error.
    pub const fn as_validation(&self) -> Option<&BlockValidationError> {
        match self {
            Self::Validation(err) => Some(err),
            _ => None,
        }
    }

    /// Returns `true` if the error is fatal.
    ///
    /// This represents an unrecoverable database related error.
    pub const fn is_fatal(&self) -> bool {
        matches!(self, Self::CanonicalCommit { .. } | Self::CanonicalRevert { .. })
    }

    /// Returns `true` if the error is a state root error.
    pub const fn is_state_root_error(&self) -> bool {
        matches!(self, Self::Validation(BlockValidationError::StateRoot(_)))
    }
}

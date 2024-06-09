//! Commonly used error types used when doing block execution.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
use core::fmt::Display;
use reth_consensus::ConsensusError;
use reth_primitives::{revm_primitives::EVMError, BlockNumHash, B256};
use reth_prune_types::PruneSegmentError;
use reth_storage_errors::provider::ProviderError;

pub mod trie;
pub use trie::{StateRootError, StorageRootError};

/// Transaction validation errors
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockValidationError {
    /// EVM error with transaction hash and message
    EVM {
        /// The hash of the transaction
        hash: B256,
        /// The EVM error.
        error: Box<EVMError<ProviderError>>,
    },
    /// Error when recovering the sender for a transaction
    SenderRecoveryError,
    /// Error when incrementing balance in post execution
    IncrementBalanceFailed,
    /// Error when the state root does not match the expected value.
    StateRoot(StateRootError),
    /// Error when transaction gas limit exceeds available block gas
    TransactionGasLimitMoreThanAvailableBlockGas {
        /// The transaction's gas limit
        transaction_gas_limit: u64,
        /// The available block gas
        block_available_gas: u64,
    },
    /// Error for pre-merge block
    BlockPreMerge {
        /// The hash of the block
        hash: B256,
    },
    /// Error for missing total difficulty
    MissingTotalDifficulty {
        /// The hash of the block
        hash: B256,
    },
    /// Error for EIP-4788 when parent beacon block root is missing
    MissingParentBeaconBlockRoot,
    /// Error for Cancun genesis block when parent beacon block root is not zero
    CancunGenesisParentBeaconBlockRootNotZero {
        /// The beacon block root
        parent_beacon_block_root: B256,
    },
    /// EVM error during [EIP-4788] beacon root contract call.
    ///
    /// [EIP-4788]: https://eips.ethereum.org/EIPS/eip-4788
    BeaconRootContractCall {
        /// The beacon block root
        parent_beacon_block_root: Box<B256>,
        /// The error message.
        message: String,
    },
    /// Provider error during the [EIP-2935] block hash account loading.
    ///
    /// [EIP-2935]: https://eips.ethereum.org/EIPS/eip-2935
    BlockHashAccountLoadingFailed(ProviderError),
    /// EVM error during withdrawal requests contract call [EIP-7002]
    ///
    /// [EIP-7002]: https://eips.ethereum.org/EIPS/eip-7002
    WithdrawalRequestsContractCall {
        /// The error message.
        message: String,
    },
    /// Error when decoding deposit requests from receipts [EIP-6110]
    ///
    /// [EIP-6110]: https://eips.ethereum.org/EIPS/eip-6110
    DepositRequestDecode(String),
}

/// `BlockExecutor` Errors
#[derive(Debug)]
pub enum BlockExecutionError {
    /// Validation error, transparently wrapping `BlockValidationError`
    Validation(BlockValidationError),
    /// Pruning error, transparently wrapping `PruneSegmentError`
    Pruning(PruneSegmentError),
    /// Consensus error, transparently wrapping `ConsensusError`
    Consensus(ConsensusError),
    /// Transaction error on revert with inner details
    CanonicalRevert {
        /// The inner error message
        inner: String,
    },
    /// Transaction error on commit with inner details
    CanonicalCommit {
        /// The inner error message
        inner: String,
    },
    /// Error when appending chain on fork is not possible
    AppendChainDoesntConnect {
        /// The tip of the current chain
        chain_tip: Box<BlockNumHash>,
        /// The fork on the other chain
        other_chain_fork: Box<BlockNumHash>,
    },
    /// Error when fetching latest block state.
    LatestBlock(ProviderError),
    /// Arbitrary Block Executor Errors
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

    /// Create a new [`BlockExecutionError::Other`] from a given message.
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

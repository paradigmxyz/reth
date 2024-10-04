//! Commonly used error types used when doing block execution.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::{boxed::Box, string::String};
use alloy_eips::BlockNumHash;
use alloy_primitives::B256;
use derive_more::{Display, From};
use reth_consensus::ConsensusError;
use reth_prune_types::PruneSegmentError;
use reth_storage_errors::provider::ProviderError;
use revm_primitives::EVMError;

pub mod trie;
pub use trie::*;

/// Transaction validation errors
#[derive(Clone, Debug, Display, Eq, PartialEq)]
pub enum BlockValidationError {
    /// EVM error with transaction hash and message
    #[display("EVM reported invalid transaction ({hash}): {error}")]
    EVM {
        /// The hash of the transaction
        hash: B256,
        /// The EVM error.
        error: Box<EVMError<ProviderError>>,
    },
    /// Error when recovering the sender for a transaction
    #[display("failed to recover sender for transaction")]
    SenderRecoveryError,
    /// Error when incrementing balance in post execution
    #[display("incrementing balance in post execution failed")]
    IncrementBalanceFailed,
    /// Error when the state root does not match the expected value.
    // #[from(ignore)]
    StateRoot(StateRootError),
    /// Error when transaction gas limit exceeds available block gas
    #[display(
        "transaction gas limit {transaction_gas_limit} is more than blocks available gas {block_available_gas}"
    )]
    TransactionGasLimitMoreThanAvailableBlockGas {
        /// The transaction's gas limit
        transaction_gas_limit: u64,
        /// The available block gas
        block_available_gas: u64,
    },
    /// Error for pre-merge block
    #[display("block {hash} is pre merge")]
    BlockPreMerge {
        /// The hash of the block
        hash: B256,
    },
    /// Error for missing total difficulty
    #[display("missing total difficulty for block {hash}")]
    MissingTotalDifficulty {
        /// The hash of the block
        hash: B256,
    },
    /// Error for EIP-4788 when parent beacon block root is missing
    #[display("EIP-4788 parent beacon block root missing for active Cancun block")]
    MissingParentBeaconBlockRoot,
    /// Error for Cancun genesis block when parent beacon block root is not zero
    #[display(
        "the parent beacon block root is not zero for Cancun genesis block: {parent_beacon_block_root}"
    )]
    CancunGenesisParentBeaconBlockRootNotZero {
        /// The beacon block root
        parent_beacon_block_root: B256,
    },
    /// EVM error during [EIP-4788] beacon root contract call.
    ///
    /// [EIP-4788]: https://eips.ethereum.org/EIPS/eip-4788
    #[display(
        "failed to apply beacon root contract call at {parent_beacon_block_root}: {message}"
    )]
    BeaconRootContractCall {
        /// The beacon block root
        parent_beacon_block_root: Box<B256>,
        /// The error message.
        message: String,
    },
    /// EVM error during [EIP-2935] blockhash contract call.
    ///
    /// [EIP-2935]: https://eips.ethereum.org/EIPS/eip-2935
    #[display("failed to apply blockhash contract call: {message}")]
    BlockHashContractCall {
        /// The error message.
        message: String,
    },
    /// EVM error during withdrawal requests contract call [EIP-7002]
    ///
    /// [EIP-7002]: https://eips.ethereum.org/EIPS/eip-7002
    #[display("failed to apply withdrawal requests contract call: {message}")]
    WithdrawalRequestsContractCall {
        /// The error message.
        message: String,
    },
    /// EVM error during consolidation requests contract call [EIP-7251]
    ///
    /// [EIP-7251]: https://eips.ethereum.org/EIPS/eip-7251
    #[display("failed to apply consolidation requests contract call: {message}")]
    ConsolidationRequestsContractCall {
        /// The error message.
        message: String,
    },
    /// Error when decoding deposit requests from receipts [EIP-6110]
    ///
    /// [EIP-6110]: https://eips.ethereum.org/EIPS/eip-6110
    #[display("failed to decode deposit requests from receipts: {_0}")]
    DepositRequestDecode(String),
}

impl From<StateRootError> for BlockValidationError {
    fn from(error: StateRootError) -> Self {
        Self::StateRoot(error)
    }
}

impl core::error::Error for BlockValidationError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::EVM { error, .. } => core::error::Error::source(error),
            Self::StateRoot(source) => core::error::Error::source(source),
            _ => Option::None,
        }
    }
}

/// `BlockExecutor` Errors
#[derive(Debug, From, Display)]
pub enum BlockExecutionError {
    /// Validation error, transparently wrapping [`BlockValidationError`]
    Validation(BlockValidationError),
    /// Consensus error, transparently wrapping [`ConsensusError`]
    Consensus(ConsensusError),
    /// Internal, i.e. non consensus or validation related Block Executor Errors
    Internal(InternalBlockExecutionError),
}

impl BlockExecutionError {
    /// Create a new [`BlockExecutionError::Internal`] variant, containing a
    /// [`InternalBlockExecutionError::Other`] error.
    #[cfg(feature = "std")]
    pub fn other<E>(error: E) -> Self
    where
        E: core::error::Error + Send + Sync + 'static,
    {
        Self::Internal(InternalBlockExecutionError::other(error))
    }

    /// Create a new [`BlockExecutionError::Internal`] variant, containing a
    /// [`InternalBlockExecutionError::Other`] error with the given message.
    #[cfg(feature = "std")]
    pub fn msg(msg: impl std::fmt::Display) -> Self {
        Self::Internal(InternalBlockExecutionError::msg(msg))
    }

    /// Returns the inner `BlockValidationError` if the error is a validation error.
    pub const fn as_validation(&self) -> Option<&BlockValidationError> {
        match self {
            Self::Validation(err) => Some(err),
            _ => None,
        }
    }

    /// Returns `true` if the error is a state root error.
    pub const fn is_state_root_error(&self) -> bool {
        matches!(self, Self::Validation(BlockValidationError::StateRoot(_)))
    }
}

impl From<ProviderError> for BlockExecutionError {
    fn from(error: ProviderError) -> Self {
        InternalBlockExecutionError::from(error).into()
    }
}

impl core::error::Error for BlockExecutionError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Validation(source) => core::error::Error::source(source),
            Self::Consensus(source) => core::error::Error::source(source),
            Self::Internal(source) => core::error::Error::source(source),
        }
    }
}

/// Internal (i.e., not validation or consensus related) `BlockExecutor` Errors
#[derive(Display, Debug, From)]
pub enum InternalBlockExecutionError {
    /// Pruning error, transparently wrapping [`PruneSegmentError`]
    #[from]
    Pruning(PruneSegmentError),
    /// Error when appending chain on fork is not possible
    #[display(
        "appending chain on fork (other_chain_fork:?) is not possible as the tip is {chain_tip:?}"
    )]
    AppendChainDoesntConnect {
        /// The tip of the current chain
        chain_tip: Box<BlockNumHash>,
        /// The fork on the other chain
        other_chain_fork: Box<BlockNumHash>,
    },
    /// Error when fetching latest block state.
    #[from]
    LatestBlock(ProviderError),
    /// Arbitrary Block Executor Errors
    Other(Box<dyn core::error::Error + Send + Sync>),
}

impl InternalBlockExecutionError {
    /// Create a new [`InternalBlockExecutionError::Other`] variant.
    #[cfg(feature = "std")]
    pub fn other<E>(error: E) -> Self
    where
        E: core::error::Error + Send + Sync + 'static,
    {
        Self::Other(Box::new(error))
    }

    /// Create a new [`InternalBlockExecutionError::Other`] from a given message.
    #[cfg(feature = "std")]
    pub fn msg(msg: impl std::fmt::Display) -> Self {
        Self::Other(msg.to_string().into())
    }
}

impl core::error::Error for InternalBlockExecutionError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Pruning(source) => core::error::Error::source(source),
            Self::LatestBlock(source) => core::error::Error::source(source),
            _ => Option::None,
        }
    }
}

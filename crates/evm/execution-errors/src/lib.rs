//! Commonly used error types used when doing block execution.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::{
    boxed::Box,
    string::{String, ToString},
};
use alloy_primitives::B256;

pub mod trie;
pub use trie::*;

/// Block validation error.
#[derive(Debug, thiserror::Error)]
pub enum BlockValidationError {
    /// EVM error with transaction hash and message.
    #[error("EVM reported invalid transaction ({hash}): {error}")]
    InvalidTx {
        /// The hash of the transaction.
        hash: B256,
        /// The EVM error.
        error: Box<dyn core::error::Error + Send + Sync + 'static>,
    },
    /// EVM error that is not a transaction validation error.
    #[error("EVM execution failed: {error}")]
    EVM {
        /// The hash of the transaction.
        hash: B256,
        /// The EVM error.
        error: Box<dyn core::error::Error + Send + Sync + 'static>,
    },
    /// Error when incrementing balance in post execution.
    #[error("incrementing balance in post execution failed")]
    IncrementBalanceFailed,
    /// Error when transaction gas limit exceeds available block gas.
    #[error(
        "transaction gas limit {transaction_gas_limit} is more than blocks available gas {block_available_gas}"
    )]
    TransactionGasLimitMoreThanAvailableBlockGas {
        /// The transaction's gas limit.
        transaction_gas_limit: u64,
        /// The available block gas.
        block_available_gas: u64,
    },
    /// Error for EIP-4788 when parent beacon block root is missing.
    #[error("EIP-4788 parent beacon block root missing for active Cancun block")]
    MissingParentBeaconBlockRoot,
    /// Error for Cancun genesis block when parent beacon block root is not zero.
    #[error(
        "the parent beacon block root is not zero for Cancun genesis block: {parent_beacon_block_root}"
    )]
    CancunGenesisParentBeaconBlockRootNotZero {
        /// The beacon block root.
        parent_beacon_block_root: B256,
    },
    /// EVM error during EIP-4788 beacon root contract call.
    #[error("failed to apply beacon root contract call at {parent_beacon_block_root}: {message}")]
    BeaconRootContractCall {
        /// The beacon block root.
        parent_beacon_block_root: Box<B256>,
        /// The error message.
        message: String,
    },
    /// EVM error during EIP-2935 blockhash contract call.
    #[error("failed to apply blockhash contract call: {message}")]
    BlockHashContractCall {
        /// The error message.
        message: String,
    },
    /// EVM error during withdrawal requests contract call.
    #[error("failed to apply withdrawal requests contract call: {message}")]
    WithdrawalRequestsContractCall {
        /// The error message.
        message: String,
    },
    /// EVM error during consolidation requests contract call.
    #[error("failed to apply consolidation requests contract call: {message}")]
    ConsolidationRequestsContractCall {
        /// The error message.
        message: String,
    },
    /// Error when decoding deposit requests from receipts.
    #[error("failed to decode deposit requests from receipts: {_0}")]
    DepositRequestDecode(String),
    /// Error when block's total gas used exceeds the block gas limit.
    #[error("block gas used exceeds block gas limit")]
    BlockGasExceeded,
    /// Arbitrary block validation errors.
    #[error(transparent)]
    Other(Box<dyn core::error::Error + Send + Sync + 'static>),
}

impl BlockValidationError {
    /// Create a new [`BlockValidationError::Other`] variant.
    pub fn other<E>(error: E) -> Self
    where
        E: core::error::Error + Send + Sync + 'static,
    {
        Self::Other(Box::new(error))
    }

    /// Create a new [`BlockValidationError::Other`] variant from a given message.
    pub fn msg(msg: impl core::fmt::Display) -> Self {
        Self::Other(msg.to_string().into())
    }
}

/// Block execution error.
#[derive(Debug, thiserror::Error)]
pub enum BlockExecutionError {
    /// Validation error.
    #[error(transparent)]
    Validation(#[from] BlockValidationError),
    /// Internal, i.e. non-consensus or validation related block executor error.
    #[error(transparent)]
    Internal(#[from] InternalBlockExecutionError),
}

impl BlockExecutionError {
    /// Create a new [`BlockExecutionError::Internal`] variant.
    pub fn other<E>(error: E) -> Self
    where
        E: core::error::Error + Send + Sync + 'static,
    {
        Self::Internal(InternalBlockExecutionError::other(error))
    }

    /// Create a new [`BlockExecutionError::Internal`] variant with the given message.
    pub fn msg(msg: impl core::fmt::Display) -> Self {
        Self::Internal(InternalBlockExecutionError::msg(msg))
    }

    /// Returns the inner validation error if this is a validation error.
    pub const fn as_validation(&self) -> Option<&BlockValidationError> {
        match self {
            Self::Validation(err) => Some(err),
            Self::Internal(_) => None,
        }
    }

    /// Returns the inner internal error if this is an internal error.
    pub const fn as_internal(&self) -> Option<&InternalBlockExecutionError> {
        match self {
            Self::Internal(err) => Some(err),
            Self::Validation(_) => None,
        }
    }
}

/// Internal block execution error.
#[derive(Debug, thiserror::Error)]
pub enum InternalBlockExecutionError {
    /// EVM error occurred when executing a transaction.
    #[error("internal EVM error occurred when executing transaction {hash}: {error}")]
    EVM {
        /// The hash of the transaction.
        hash: B256,
        /// The EVM error.
        error: Box<dyn core::error::Error + Send + Sync + 'static>,
    },
    /// Arbitrary block executor errors.
    #[error(transparent)]
    Other(Box<dyn core::error::Error + Send + Sync + 'static>),
}

impl InternalBlockExecutionError {
    /// Create a new [`InternalBlockExecutionError::Other`] variant.
    pub fn other<E>(error: E) -> Self
    where
        E: core::error::Error + Send + Sync + 'static,
    {
        Self::Other(Box::new(error))
    }

    /// Create a new [`InternalBlockExecutionError::Other`] from a given message.
    pub fn msg(msg: impl core::fmt::Display) -> Self {
        Self::Other(msg.to_string().into())
    }

    /// Returns the arbitrary error if this is [`InternalBlockExecutionError::Other`].
    pub fn as_other(&self) -> Option<&(dyn core::error::Error + Send + Sync + 'static)> {
        match self {
            Self::Other(err) => Some(&**err),
            Self::EVM { .. } => None,
        }
    }

    /// Attempts to downcast the [`InternalBlockExecutionError::Other`] variant to a concrete type.
    pub fn downcast<T: core::error::Error + 'static>(self) -> Result<Box<T>, Self> {
        match self {
            Self::Other(err) => err.downcast().map_err(Self::Other),
            err => Err(err),
        }
    }

    /// Returns a reference to the [`InternalBlockExecutionError::Other`] value if it has type `T`.
    pub fn downcast_other<T: core::error::Error + 'static>(&self) -> Option<&T> {
        self.as_other()?.downcast_ref()
    }

    /// Returns true if this is an [`InternalBlockExecutionError::Other`] of type `T`.
    pub fn is_other<T: core::error::Error + 'static>(&self) -> bool {
        self.as_other().map(|err| err.is::<T>()).unwrap_or(false)
    }

    /// Returns the EVM error and transaction hash if this is [`InternalBlockExecutionError::EVM`].
    pub fn as_evm(&self) -> Option<(&B256, &(dyn core::error::Error + Send + Sync + 'static))> {
        match self {
            Self::EVM { hash, error } => Some((hash, &**error)),
            Self::Other(_) => None,
        }
    }
}

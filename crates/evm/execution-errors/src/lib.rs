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

use alloc::{
    boxed::Box,
    string::{String, ToString},
};
use alloy_evm::{EvmError, InvalidTxError};
use alloy_primitives::B256;
use reth_storage_errors::provider::ProviderError;
use thiserror::Error;

pub mod trie;
pub use trie::*;

/// Transaction validation errors
#[derive(Error, Debug)]
pub enum BlockValidationError {
    /// EVM error with transaction hash and message
    #[error("EVM reported invalid transaction ({hash}): {error}")]
    InvalidTx {
        /// The hash of the transaction
        hash: B256,
        /// The EVM error.
        error: Box<dyn InvalidTxError>,
    },
    /// Error when incrementing balance in post execution
    #[error("incrementing balance in post execution failed")]
    IncrementBalanceFailed,
    /// Error when transaction gas limit exceeds available block gas
    #[error(
        "transaction gas limit {transaction_gas_limit} is more than blocks available gas {block_available_gas}"
    )]
    TransactionGasLimitMoreThanAvailableBlockGas {
        /// The transaction's gas limit
        transaction_gas_limit: u64,
        /// The available block gas
        block_available_gas: u64,
    },
    /// Error for EIP-4788 when parent beacon block root is missing
    #[error("EIP-4788 parent beacon block root missing for active Cancun block")]
    MissingParentBeaconBlockRoot,
    /// Error for Cancun genesis block when parent beacon block root is not zero
    #[error(
        "the parent beacon block root is not zero for Cancun genesis block: {parent_beacon_block_root}"
    )]
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
    /// EVM error during [EIP-2935] blockhash contract call.
    ///
    /// [EIP-2935]: https://eips.ethereum.org/EIPS/eip-2935
    #[error("failed to apply blockhash contract call: {message}")]
    BlockHashContractCall {
        /// The error message.
        message: String,
    },
    /// EVM error during withdrawal requests contract call [EIP-7002]
    ///
    /// [EIP-7002]: https://eips.ethereum.org/EIPS/eip-7002
    #[error("failed to apply withdrawal requests contract call: {message}")]
    WithdrawalRequestsContractCall {
        /// The error message.
        message: String,
    },
    /// EVM error during consolidation requests contract call [EIP-7251]
    ///
    /// [EIP-7251]: https://eips.ethereum.org/EIPS/eip-7251
    #[error("failed to apply consolidation requests contract call: {message}")]
    ConsolidationRequestsContractCall {
        /// The error message.
        message: String,
    },
    /// Error when decoding deposit requests from receipts [EIP-6110]
    ///
    /// [EIP-6110]: https://eips.ethereum.org/EIPS/eip-6110
    #[error("failed to decode deposit requests from receipts: {_0}")]
    DepositRequestDecode(String),
}

/// `BlockExecutor` Errors
#[derive(Error, Debug)]
pub enum BlockExecutionError {
    /// Validation error, transparently wrapping [`BlockValidationError`]
    #[error(transparent)]
    Validation(#[from] BlockValidationError),
    /// Internal, i.e. non consensus or validation related Block Executor Errors
    #[error(transparent)]
    Internal(#[from] InternalBlockExecutionError),
}

impl BlockExecutionError {
    /// Create a new [`BlockExecutionError::Internal`] variant, containing a
    /// [`InternalBlockExecutionError::Other`] error.
    pub fn other<E>(error: E) -> Self
    where
        E: core::error::Error + Send + Sync + 'static,
    {
        Self::Internal(InternalBlockExecutionError::other(error))
    }

    /// Create a new [`BlockExecutionError::Internal`] variant, containing a
    /// [`InternalBlockExecutionError::Other`] error with the given message.
    pub fn msg(msg: impl core::fmt::Display) -> Self {
        Self::Internal(InternalBlockExecutionError::msg(msg))
    }

    /// Returns the inner `BlockValidationError` if the error is a validation error.
    pub const fn as_validation(&self) -> Option<&BlockValidationError> {
        match self {
            Self::Validation(err) => Some(err),
            _ => None,
        }
    }

    /// Handles an EVM error occurred when executing a transaction.
    ///
    /// If an error matches [`EvmError::InvalidTransaction`], it will be wrapped into
    /// [`BlockValidationError::InvalidTx`], otherwise into [`InternalBlockExecutionError::EVM`].
    pub fn evm<E: EvmError>(error: E, hash: B256) -> Self {
        match error.try_into_invalid_tx_err() {
            Ok(err) => {
                Self::Validation(BlockValidationError::InvalidTx { hash, error: Box::new(err) })
            }
            Err(err) => {
                Self::Internal(InternalBlockExecutionError::EVM { hash, error: Box::new(err) })
            }
        }
    }
}

impl From<ProviderError> for BlockExecutionError {
    fn from(error: ProviderError) -> Self {
        Self::other(error)
    }
}

/// Internal (i.e., not validation or consensus related) `BlockExecutor` Errors
#[derive(Error, Debug)]
pub enum InternalBlockExecutionError {
    /// EVM error occurred when executing transaction. This is different from
    /// [`BlockValidationError::InvalidTx`] because it will only contain EVM errors which are not
    /// transaction validation errors and are assumed to be fatal.
    #[error("internal EVM error occurred when executing transaction {hash}: {error}")]
    EVM {
        /// The hash of the transaction
        hash: B256,
        /// The EVM error.
        error: Box<dyn core::error::Error + Send + Sync>,
    },
    /// Arbitrary Block Executor Errors
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

    /// Returns the arbitrary error if it is [`InternalBlockExecutionError::Other`]
    pub fn as_other(&self) -> Option<&(dyn core::error::Error + Send + Sync + 'static)> {
        match self {
            Self::Other(err) => Some(&**err),
            _ => None,
        }
    }

    /// Attempts to downcast the [`InternalBlockExecutionError::Other`] variant to a concrete type
    pub fn downcast<T: core::error::Error + 'static>(self) -> Result<Box<T>, Self> {
        match self {
            Self::Other(err) => err.downcast().map_err(Self::Other),
            err => Err(err),
        }
    }

    /// Returns a reference to the [`InternalBlockExecutionError::Other`] value if this type is a
    /// [`InternalBlockExecutionError::Other`] of that type. Returns None otherwise.
    pub fn downcast_other<T: core::error::Error + 'static>(&self) -> Option<&T> {
        let other = self.as_other()?;
        other.downcast_ref()
    }

    /// Returns true if the this type is a [`InternalBlockExecutionError::Other`] of that error
    /// type. Returns false otherwise.
    pub fn is_other<T: core::error::Error + 'static>(&self) -> bool {
        self.as_other().map(|err| err.is::<T>()).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(thiserror::Error, Debug)]
    #[error("err")]
    struct E;

    #[test]
    fn other_downcast() {
        let err = InternalBlockExecutionError::other(E);
        assert!(err.is_other::<E>());

        assert!(err.downcast_other::<E>().is_some());
        assert!(err.downcast::<E>().is_ok());
    }
}

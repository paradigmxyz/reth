//! Commonly used error types used when doing block execution.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
use core::fmt::{Display, Formatter, Result};
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

#[cfg(feature = "std")]
impl std::error::Error for BlockValidationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        #[allow(deprecated)]
        match self {
            Self::EVM { error: source, .. } => ::core::option::Option::Some(source),
            Self::StateRoot { 0: transparent } => std::error::Error::source(transparent),
            Self::BlockHashAccountLoadingFailed { 0: transparent } => {
                std::error::Error::source(transparent)
            }
            _ => None,
        }
    }
}

impl Display for BlockValidationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        #[allow(unused_variables, deprecated, clippy::used_underscore_binding)]
        match self {
            Self::EVM { hash, error } => {
                write!(f, "EVM reported invalid transaction ({}): {}", hash, error,)
            }
            Self::SenderRecoveryError => f.write_str("failed to recover sender for transaction"),
            Self::IncrementBalanceFailed => {
                f.write_str("incrementing balance in post execution failed")
            }
            Self::StateRoot(state_root_error) => Display::fmt(state_root_error, f),
            Self::TransactionGasLimitMoreThanAvailableBlockGas {
                transaction_gas_limit,
                block_available_gas,
            } => write!(
                f,
                "transaction gas limit {} is more than block's available gas {}",
                transaction_gas_limit, block_available_gas,
            ),
            Self::BlockPreMerge { hash } => {
                write!(f, "block {} is pre merge", hash)
            }
            Self::MissingTotalDifficulty { hash } => {
                write!(f, "missing total difficulty for block {}", hash)
            }
            Self::MissingParentBeaconBlockRoot => {
                f.write_str("EIP-4788 parent beacon block root missing for active Cancun block")
            }
            Self::CancunGenesisParentBeaconBlockRootNotZero { parent_beacon_block_root } => {
                write!(
                    f,
                    "the parent beacon block root is not zero for Cancun genesis block: {}",
                    parent_beacon_block_root,
                )
            }
            Self::BeaconRootContractCall { parent_beacon_block_root, message } => write!(
                f,
                "failed to apply beacon root contract call at {}: {}",
                parent_beacon_block_root, message,
            ),
            Self::BlockHashAccountLoadingFailed(account_loading_error) => {
                Display::fmt(account_loading_error, f)
            }
            Self::WithdrawalRequestsContractCall { message } => {
                write!(f, "failed to apply withdrawal requests contract call: {}", message,)
            }
            Self::DepositRequestDecode(deposit_request) => {
                write!(f, "failed to decode deposit requests from receipts: {}", deposit_request,)
            }
        }
    }
}

impl From<StateRootError> for BlockValidationError {
    fn from(source: StateRootError) -> Self {
        Self::StateRoot(source)
    }
}
impl From<ProviderError> for BlockValidationError {
    fn from(source: ProviderError) -> Self {
        Self::BlockHashAccountLoadingFailed(source)
    }
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

#[cfg(feature = "std")]
impl std::error::Error for BlockExecutionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Validation(validation_error) => Some(validation_error),
            Self::Pruning(pruning_error) => Some(pruning_error),
            Self::Consensus(consensus_error) => Some(consensus_error),
            Self::LatestBlock(latest_block_error) => Some(latest_block_error),
            Self::Other(other_error) => Some(&**other_error),
            _ => None,
        }
    }
}

impl Display for BlockExecutionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            Self::Validation(validation_error) => Display::fmt(validation_error, f),
            Self::Pruning(pruning_error) => Display::fmt(pruning_error, f),
            Self::Consensus(consensus_error) => Display::fmt(consensus_error, f),
            Self::CanonicalRevert { inner } => {
                write!(f, "transaction error on revert: {}", inner)
            }
            Self::CanonicalCommit { inner } => {
                write!(f, "transaction error on commit: {}", inner)
            }
            Self::AppendChainDoesntConnect { chain_tip, other_chain_fork } => {
                write!(
                    f,
                    "appending chain on fork {:?} is not possible as the tip is {:?}",
                    other_chain_fork, chain_tip,
                )
            }
            Self::LatestBlock(latest_block_error) => Display::fmt(latest_block_error, f),
            Self::Other(other_error) => Display::fmt(other_error, f),
        }
    }
}

impl From<BlockValidationError> for BlockExecutionError {
    fn from(source: BlockValidationError) -> Self {
        Self::Validation(source)
    }
}
#[allow(unused_qualifications)]
impl From<PruneSegmentError> for BlockExecutionError {
    fn from(source: PruneSegmentError) -> Self {
        Self::Pruning(source)
    }
}
#[allow(unused_qualifications)]
impl From<ConsensusError> for BlockExecutionError {
    fn from(source: ConsensusError) -> Self {
        Self::Consensus(source)
    }
}
#[allow(unused_qualifications)]
impl From<ProviderError> for BlockExecutionError {
    fn from(source: ProviderError) -> Self {
        Self::LatestBlock(source)
    }
}

impl BlockExecutionError {
    /// Create a new `Self::Other` variant.
    pub fn other<E>(error: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Other(Box::new(error))
    }

    /// Create a new [`Self::Other`] from a given message.
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

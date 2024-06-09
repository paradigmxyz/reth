#![feature(prelude_import)]
//! Commonly used error types used when doing block execution.
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![warn(unused_crate_dependencies)]
#[prelude_import]
use std::prelude::rust_2021::*;
#[macro_use]
extern crate std;
use reth_consensus::ConsensusError;
use reth_primitives::{revm_primitives::EVMError, BlockNumHash, B256};
use reth_prune_types::PruneSegmentError;
use reth_storage_errors::provider::ProviderError;
use std::fmt::Display;
use thiserror::Error;
pub mod trie {
    //! Errors when computing the state root.
    use core::fmt::{Display, Formatter, Result};
    use reth_storage_errors::db::DatabaseError;
    /// State root errors.
    pub enum StateRootError {
        /// Internal database error.
        DB(DatabaseError),
        /// Storage root error.
        StorageRootError(StorageRootError),
    }
    #[automatically_derived]
    impl ::core::fmt::Debug for StateRootError {
        #[inline]
        fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
            match self {
                StateRootError::DB(__self_0) => {
                    ::core::fmt::Formatter::debug_tuple_field1_finish(f, "DB", &__self_0)
                }
                StateRootError::StorageRootError(__self_0) => {
                    ::core::fmt::Formatter::debug_tuple_field1_finish(
                        f,
                        "StorageRootError",
                        &__self_0,
                    )
                }
            }
        }
    }
    #[automatically_derived]
    impl ::core::marker::StructuralPartialEq for StateRootError {}
    #[automatically_derived]
    impl ::core::cmp::PartialEq for StateRootError {
        #[inline]
        fn eq(&self, other: &StateRootError) -> bool {
            let __self_tag = ::core::intrinsics::discriminant_value(self);
            let __arg1_tag = ::core::intrinsics::discriminant_value(other);
            __self_tag == __arg1_tag
                && match (self, other) {
                    (StateRootError::DB(__self_0), StateRootError::DB(__arg1_0)) => {
                        *__self_0 == *__arg1_0
                    }
                    (
                        StateRootError::StorageRootError(__self_0),
                        StateRootError::StorageRootError(__arg1_0),
                    ) => *__self_0 == *__arg1_0,
                    _ => unsafe { ::core::intrinsics::unreachable() }
                }
        }
    }
    #[automatically_derived]
    impl ::core::cmp::Eq for StateRootError {
        #[inline]
        #[doc(hidden)]
        #[coverage(off)]
        fn assert_receiver_is_total_eq(&self) -> () {
            let _: ::core::cmp::AssertParamIsEq<DatabaseError>;
            let _: ::core::cmp::AssertParamIsEq<StorageRootError>;
        }
    }
    #[automatically_derived]
    impl ::core::clone::Clone for StateRootError {
        #[inline]
        fn clone(&self) -> StateRootError {
            match self {
                StateRootError::DB(__self_0) => {
                    StateRootError::DB(::core::clone::Clone::clone(__self_0))
                }
                StateRootError::StorageRootError(__self_0) => {
                    StateRootError::StorageRootError(
                        ::core::clone::Clone::clone(__self_0),
                    )
                }
            }
        }
    }
    #[cfg(feature = "std")]
    impl std::error::Error for StateRootError {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            match self {
                Self::DB(db_err) => Some(db_err),
                Self::StorageRootError(storage_err) => Some(storage_err),
            }
        }
    }
    impl Display for StateRootError {
        fn fmt(&self, f: &mut Formatter<'_>) -> Result {
            match self {
                Self::DB(db_err) => Display::fmt(db_err, f),
                Self::StorageRootError(storage_err) => Display::fmt(storage_err, f),
            }
        }
    }
    impl From<DatabaseError> for StateRootError {
        fn from(err: DatabaseError) -> Self {
            Self::DB(err)
        }
    }
    impl From<StorageRootError> for StateRootError {
        fn from(err: StorageRootError) -> Self {
            Self::StorageRootError(err)
        }
    }
    impl From<StateRootError> for DatabaseError {
        fn from(err: StateRootError) -> Self {
            match err {
                StateRootError::DB(db_err) => db_err,
                StateRootError::StorageRootError(storage_err) => {
                    match storage_err {
                        StorageRootError::DB(db_err) => db_err,
                    }
                }
            }
        }
    }
    /// Storage root error.
    pub enum StorageRootError {
        /// Internal database error
        DB(DatabaseError),
    }
    #[automatically_derived]
    impl ::core::marker::StructuralPartialEq for StorageRootError {}
    #[automatically_derived]
    impl ::core::cmp::PartialEq for StorageRootError {
        #[inline]
        fn eq(&self, other: &StorageRootError) -> bool {
            match (self, other) {
                (StorageRootError::DB(__self_0), StorageRootError::DB(__arg1_0)) => {
                    *__self_0 == *__arg1_0
                }
            }
        }
    }
    #[automatically_derived]
    impl ::core::cmp::Eq for StorageRootError {
        #[inline]
        #[doc(hidden)]
        #[coverage(off)]
        fn assert_receiver_is_total_eq(&self) -> () {
            let _: ::core::cmp::AssertParamIsEq<DatabaseError>;
        }
    }
    #[automatically_derived]
    impl ::core::clone::Clone for StorageRootError {
        #[inline]
        fn clone(&self) -> StorageRootError {
            match self {
                StorageRootError::DB(__self_0) => {
                    StorageRootError::DB(::core::clone::Clone::clone(__self_0))
                }
            }
        }
    }
    #[automatically_derived]
    impl ::core::fmt::Debug for StorageRootError {
        #[inline]
        fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
            match self {
                StorageRootError::DB(__self_0) => {
                    ::core::fmt::Formatter::debug_tuple_field1_finish(f, "DB", &__self_0)
                }
            }
        }
    }
    #[cfg(feature = "std")]
    impl std::error::Error for StorageRootError {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            match self {
                Self::DB(db_err) => Some(db_err),
            }
        }
    }
    #[allow(unused_qualifications)]
    impl Display for StorageRootError {
        fn fmt(&self, f: &mut Formatter<'_>) -> Result {
            match self {
                Self::DB(db_err) => Display::fmt(db_err, f),
            }
        }
    }
    impl From<DatabaseError> for StorageRootError {
        fn from(err: DatabaseError) -> Self {
            Self::DB(err)
        }
    }
}
pub use trie::{StateRootError, StorageRootError};
/// Transaction validation errors
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
    #[error(
        "transaction gas limit {transaction_gas_limit} is more than blocks available gas {block_available_gas}"
    )]
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
    #[error(
        "failed to apply beacon root contract call at {parent_beacon_block_root}: {message}"
    )]
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
#[allow(unused_qualifications)]
impl std::error::Error for BlockValidationError {
    fn source(&self) -> ::core::option::Option<&(dyn std::error::Error + 'static)> {
        use thiserror::__private::AsDynError as _;
        #[allow(deprecated)]
        match self {
            BlockValidationError::EVM { error: source, .. } => {
                ::core::option::Option::Some(source.as_dyn_error())
            }
            BlockValidationError::SenderRecoveryError { .. } => {
                ::core::option::Option::None
            }
            BlockValidationError::IncrementBalanceFailed { .. } => {
                ::core::option::Option::None
            }
            BlockValidationError::StateRoot { 0: transparent } => {
                std::error::Error::source(transparent.as_dyn_error())
            }
            BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas { .. } => {
                ::core::option::Option::None
            }
            BlockValidationError::BlockPreMerge { .. } => ::core::option::Option::None,
            BlockValidationError::MissingTotalDifficulty { .. } => {
                ::core::option::Option::None
            }
            BlockValidationError::MissingParentBeaconBlockRoot { .. } => {
                ::core::option::Option::None
            }
            BlockValidationError::CancunGenesisParentBeaconBlockRootNotZero { .. } => {
                ::core::option::Option::None
            }
            BlockValidationError::BeaconRootContractCall { .. } => {
                ::core::option::Option::None
            }
            BlockValidationError::BlockHashAccountLoadingFailed { 0: transparent } => {
                std::error::Error::source(transparent.as_dyn_error())
            }
            BlockValidationError::WithdrawalRequestsContractCall { .. } => {
                ::core::option::Option::None
            }
            BlockValidationError::DepositRequestDecode { .. } => {
                ::core::option::Option::None
            }
        }
    }
}
#[allow(unused_qualifications)]
impl ::core::fmt::Display for BlockValidationError {
    fn fmt(&self, __formatter: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
        use thiserror::__private::AsDisplay as _;
        #[allow(unused_variables, deprecated, clippy::used_underscore_binding)]
        match self {
            BlockValidationError::EVM { hash, error } => {
                __formatter
                    .write_fmt(
                        format_args!(
                            "EVM reported invalid transaction ({0}): {1}",
                            hash.as_display(),
                            error.as_display(),
                        ),
                    )
            }
            BlockValidationError::SenderRecoveryError {} => {
                __formatter.write_str("failed to recover sender for transaction")
            }
            BlockValidationError::IncrementBalanceFailed {} => {
                __formatter.write_str("incrementing balance in post execution failed")
            }
            BlockValidationError::StateRoot(_0) => {
                ::core::fmt::Display::fmt(_0, __formatter)
            }
            BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                transaction_gas_limit,
                block_available_gas,
            } => {
                __formatter
                    .write_fmt(
                        format_args!(
                            "transaction gas limit {0} is more than blocks available gas {1}",
                            transaction_gas_limit.as_display(),
                            block_available_gas.as_display(),
                        ),
                    )
            }
            BlockValidationError::BlockPreMerge { hash } => {
                __formatter
                    .write_fmt(format_args!("block {0} is pre merge", hash.as_display()))
            }
            BlockValidationError::MissingTotalDifficulty { hash } => {
                __formatter
                    .write_fmt(
                        format_args!(
                            "missing total difficulty for block {0}",
                            hash.as_display(),
                        ),
                    )
            }
            BlockValidationError::MissingParentBeaconBlockRoot {} => {
                __formatter
                    .write_str(
                        "EIP-4788 parent beacon block root missing for active Cancun block",
                    )
            }
            BlockValidationError::CancunGenesisParentBeaconBlockRootNotZero {
                parent_beacon_block_root,
            } => {
                __formatter
                    .write_fmt(
                        format_args!(
                            "the parent beacon block root is not zero for Cancun genesis block: {0}",
                            parent_beacon_block_root.as_display(),
                        ),
                    )
            }
            BlockValidationError::BeaconRootContractCall {
                parent_beacon_block_root,
                message,
            } => {
                __formatter
                    .write_fmt(
                        format_args!(
                            "failed to apply beacon root contract call at {0}: {1}",
                            parent_beacon_block_root.as_display(),
                            message.as_display(),
                        ),
                    )
            }
            BlockValidationError::BlockHashAccountLoadingFailed(_0) => {
                ::core::fmt::Display::fmt(_0, __formatter)
            }
            BlockValidationError::WithdrawalRequestsContractCall { message } => {
                __formatter
                    .write_fmt(
                        format_args!(
                            "failed to apply withdrawal requests contract call: {0}",
                            message.as_display(),
                        ),
                    )
            }
            BlockValidationError::DepositRequestDecode(_0) => {
                __formatter
                    .write_fmt(
                        format_args!(
                            "failed to decode deposit requests from receipts: {0}",
                            _0.as_display(),
                        ),
                    )
            }
        }
    }
}
#[allow(unused_qualifications)]
impl ::core::convert::From<StateRootError> for BlockValidationError {
    #[allow(deprecated)]
    fn from(source: StateRootError) -> Self {
        BlockValidationError::StateRoot {
            0: source,
        }
    }
}
#[allow(unused_qualifications)]
impl ::core::convert::From<ProviderError> for BlockValidationError {
    #[allow(deprecated)]
    fn from(source: ProviderError) -> Self {
        BlockValidationError::BlockHashAccountLoadingFailed {
            0: source,
        }
    }
}
#[automatically_derived]
impl ::core::fmt::Debug for BlockValidationError {
    #[inline]
    fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
        match self {
            BlockValidationError::EVM { hash: __self_0, error: __self_1 } => {
                ::core::fmt::Formatter::debug_struct_field2_finish(
                    f,
                    "EVM",
                    "hash",
                    __self_0,
                    "error",
                    &__self_1,
                )
            }
            BlockValidationError::SenderRecoveryError => {
                ::core::fmt::Formatter::write_str(f, "SenderRecoveryError")
            }
            BlockValidationError::IncrementBalanceFailed => {
                ::core::fmt::Formatter::write_str(f, "IncrementBalanceFailed")
            }
            BlockValidationError::StateRoot(__self_0) => {
                ::core::fmt::Formatter::debug_tuple_field1_finish(
                    f,
                    "StateRoot",
                    &__self_0,
                )
            }
            BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                transaction_gas_limit: __self_0,
                block_available_gas: __self_1,
            } => {
                ::core::fmt::Formatter::debug_struct_field2_finish(
                    f,
                    "TransactionGasLimitMoreThanAvailableBlockGas",
                    "transaction_gas_limit",
                    __self_0,
                    "block_available_gas",
                    &__self_1,
                )
            }
            BlockValidationError::BlockPreMerge { hash: __self_0 } => {
                ::core::fmt::Formatter::debug_struct_field1_finish(
                    f,
                    "BlockPreMerge",
                    "hash",
                    &__self_0,
                )
            }
            BlockValidationError::MissingTotalDifficulty { hash: __self_0 } => {
                ::core::fmt::Formatter::debug_struct_field1_finish(
                    f,
                    "MissingTotalDifficulty",
                    "hash",
                    &__self_0,
                )
            }
            BlockValidationError::MissingParentBeaconBlockRoot => {
                ::core::fmt::Formatter::write_str(f, "MissingParentBeaconBlockRoot")
            }
            BlockValidationError::CancunGenesisParentBeaconBlockRootNotZero {
                parent_beacon_block_root: __self_0,
            } => {
                ::core::fmt::Formatter::debug_struct_field1_finish(
                    f,
                    "CancunGenesisParentBeaconBlockRootNotZero",
                    "parent_beacon_block_root",
                    &__self_0,
                )
            }
            BlockValidationError::BeaconRootContractCall {
                parent_beacon_block_root: __self_0,
                message: __self_1,
            } => {
                ::core::fmt::Formatter::debug_struct_field2_finish(
                    f,
                    "BeaconRootContractCall",
                    "parent_beacon_block_root",
                    __self_0,
                    "message",
                    &__self_1,
                )
            }
            BlockValidationError::BlockHashAccountLoadingFailed(__self_0) => {
                ::core::fmt::Formatter::debug_tuple_field1_finish(
                    f,
                    "BlockHashAccountLoadingFailed",
                    &__self_0,
                )
            }
            BlockValidationError::WithdrawalRequestsContractCall {
                message: __self_0,
            } => {
                ::core::fmt::Formatter::debug_struct_field1_finish(
                    f,
                    "WithdrawalRequestsContractCall",
                    "message",
                    &__self_0,
                )
            }
            BlockValidationError::DepositRequestDecode(__self_0) => {
                ::core::fmt::Formatter::debug_tuple_field1_finish(
                    f,
                    "DepositRequestDecode",
                    &__self_0,
                )
            }
        }
    }
}
#[automatically_derived]
impl ::core::clone::Clone for BlockValidationError {
    #[inline]
    fn clone(&self) -> BlockValidationError {
        match self {
            BlockValidationError::EVM { hash: __self_0, error: __self_1 } => {
                BlockValidationError::EVM {
                    hash: ::core::clone::Clone::clone(__self_0),
                    error: ::core::clone::Clone::clone(__self_1),
                }
            }
            BlockValidationError::SenderRecoveryError => {
                BlockValidationError::SenderRecoveryError
            }
            BlockValidationError::IncrementBalanceFailed => {
                BlockValidationError::IncrementBalanceFailed
            }
            BlockValidationError::StateRoot(__self_0) => {
                BlockValidationError::StateRoot(::core::clone::Clone::clone(__self_0))
            }
            BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                transaction_gas_limit: __self_0,
                block_available_gas: __self_1,
            } => {
                BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                    transaction_gas_limit: ::core::clone::Clone::clone(__self_0),
                    block_available_gas: ::core::clone::Clone::clone(__self_1),
                }
            }
            BlockValidationError::BlockPreMerge { hash: __self_0 } => {
                BlockValidationError::BlockPreMerge {
                    hash: ::core::clone::Clone::clone(__self_0),
                }
            }
            BlockValidationError::MissingTotalDifficulty { hash: __self_0 } => {
                BlockValidationError::MissingTotalDifficulty {
                    hash: ::core::clone::Clone::clone(__self_0),
                }
            }
            BlockValidationError::MissingParentBeaconBlockRoot => {
                BlockValidationError::MissingParentBeaconBlockRoot
            }
            BlockValidationError::CancunGenesisParentBeaconBlockRootNotZero {
                parent_beacon_block_root: __self_0,
            } => {
                BlockValidationError::CancunGenesisParentBeaconBlockRootNotZero {
                    parent_beacon_block_root: ::core::clone::Clone::clone(__self_0),
                }
            }
            BlockValidationError::BeaconRootContractCall {
                parent_beacon_block_root: __self_0,
                message: __self_1,
            } => {
                BlockValidationError::BeaconRootContractCall {
                    parent_beacon_block_root: ::core::clone::Clone::clone(__self_0),
                    message: ::core::clone::Clone::clone(__self_1),
                }
            }
            BlockValidationError::BlockHashAccountLoadingFailed(__self_0) => {
                BlockValidationError::BlockHashAccountLoadingFailed(
                    ::core::clone::Clone::clone(__self_0),
                )
            }
            BlockValidationError::WithdrawalRequestsContractCall {
                message: __self_0,
            } => {
                BlockValidationError::WithdrawalRequestsContractCall {
                    message: ::core::clone::Clone::clone(__self_0),
                }
            }
            BlockValidationError::DepositRequestDecode(__self_0) => {
                BlockValidationError::DepositRequestDecode(
                    ::core::clone::Clone::clone(__self_0),
                )
            }
        }
    }
}
#[automatically_derived]
impl ::core::marker::StructuralPartialEq for BlockValidationError {}
#[automatically_derived]
impl ::core::cmp::PartialEq for BlockValidationError {
    #[inline]
    fn eq(&self, other: &BlockValidationError) -> bool {
        let __self_tag = ::core::intrinsics::discriminant_value(self);
        let __arg1_tag = ::core::intrinsics::discriminant_value(other);
        __self_tag == __arg1_tag
            && match (self, other) {
                (
                    BlockValidationError::EVM { hash: __self_0, error: __self_1 },
                    BlockValidationError::EVM { hash: __arg1_0, error: __arg1_1 },
                ) => *__self_0 == *__arg1_0 && *__self_1 == *__arg1_1,
                (
                    BlockValidationError::StateRoot(__self_0),
                    BlockValidationError::StateRoot(__arg1_0),
                ) => *__self_0 == *__arg1_0,
                (
                    BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                        transaction_gas_limit: __self_0,
                        block_available_gas: __self_1,
                    },
                    BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                        transaction_gas_limit: __arg1_0,
                        block_available_gas: __arg1_1,
                    },
                ) => *__self_0 == *__arg1_0 && *__self_1 == *__arg1_1,
                (
                    BlockValidationError::BlockPreMerge { hash: __self_0 },
                    BlockValidationError::BlockPreMerge { hash: __arg1_0 },
                ) => *__self_0 == *__arg1_0,
                (
                    BlockValidationError::MissingTotalDifficulty { hash: __self_0 },
                    BlockValidationError::MissingTotalDifficulty { hash: __arg1_0 },
                ) => *__self_0 == *__arg1_0,
                (
                    BlockValidationError::CancunGenesisParentBeaconBlockRootNotZero {
                        parent_beacon_block_root: __self_0,
                    },
                    BlockValidationError::CancunGenesisParentBeaconBlockRootNotZero {
                        parent_beacon_block_root: __arg1_0,
                    },
                ) => *__self_0 == *__arg1_0,
                (
                    BlockValidationError::BeaconRootContractCall {
                        parent_beacon_block_root: __self_0,
                        message: __self_1,
                    },
                    BlockValidationError::BeaconRootContractCall {
                        parent_beacon_block_root: __arg1_0,
                        message: __arg1_1,
                    },
                ) => *__self_0 == *__arg1_0 && *__self_1 == *__arg1_1,
                (
                    BlockValidationError::BlockHashAccountLoadingFailed(__self_0),
                    BlockValidationError::BlockHashAccountLoadingFailed(__arg1_0),
                ) => *__self_0 == *__arg1_0,
                (
                    BlockValidationError::WithdrawalRequestsContractCall {
                        message: __self_0,
                    },
                    BlockValidationError::WithdrawalRequestsContractCall {
                        message: __arg1_0,
                    },
                ) => *__self_0 == *__arg1_0,
                (
                    BlockValidationError::DepositRequestDecode(__self_0),
                    BlockValidationError::DepositRequestDecode(__arg1_0),
                ) => *__self_0 == *__arg1_0,
                _ => true,
            }
    }
}
#[automatically_derived]
impl ::core::cmp::Eq for BlockValidationError {
    #[inline]
    #[doc(hidden)]
    #[coverage(off)]
    fn assert_receiver_is_total_eq(&self) -> () {
        let _: ::core::cmp::AssertParamIsEq<B256>;
        let _: ::core::cmp::AssertParamIsEq<Box<EVMError<ProviderError>>>;
        let _: ::core::cmp::AssertParamIsEq<StateRootError>;
        let _: ::core::cmp::AssertParamIsEq<u64>;
        let _: ::core::cmp::AssertParamIsEq<Box<B256>>;
        let _: ::core::cmp::AssertParamIsEq<String>;
        let _: ::core::cmp::AssertParamIsEq<ProviderError>;
    }
}
/// `BlockExecutor` Errors
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
#[allow(unused_qualifications)]
impl std::error::Error for BlockExecutionError {
    fn source(&self) -> ::core::option::Option<&(dyn std::error::Error + 'static)> {
        use thiserror::__private::AsDynError as _;
        #[allow(deprecated)]
        match self {
            BlockExecutionError::Validation { 0: transparent } => {
                std::error::Error::source(transparent.as_dyn_error())
            }
            BlockExecutionError::Pruning { 0: transparent } => {
                std::error::Error::source(transparent.as_dyn_error())
            }
            BlockExecutionError::Consensus { 0: transparent } => {
                std::error::Error::source(transparent.as_dyn_error())
            }
            BlockExecutionError::CanonicalRevert { .. } => ::core::option::Option::None,
            BlockExecutionError::CanonicalCommit { .. } => ::core::option::Option::None,
            BlockExecutionError::AppendChainDoesntConnect { .. } => {
                ::core::option::Option::None
            }
            BlockExecutionError::LatestBlock { 0: transparent } => {
                std::error::Error::source(transparent.as_dyn_error())
            }
            BlockExecutionError::Other { 0: transparent } => {
                std::error::Error::source(transparent.as_dyn_error())
            }
        }
    }
}
#[allow(unused_qualifications)]
impl ::core::fmt::Display for BlockExecutionError {
    fn fmt(&self, __formatter: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
        use thiserror::__private::AsDisplay as _;
        #[allow(unused_variables, deprecated, clippy::used_underscore_binding)]
        match self {
            BlockExecutionError::Validation(_0) => {
                ::core::fmt::Display::fmt(_0, __formatter)
            }
            BlockExecutionError::Pruning(_0) => {
                ::core::fmt::Display::fmt(_0, __formatter)
            }
            BlockExecutionError::Consensus(_0) => {
                ::core::fmt::Display::fmt(_0, __formatter)
            }
            BlockExecutionError::CanonicalRevert { inner } => {
                __formatter
                    .write_fmt(
                        format_args!(
                            "transaction error on revert: {0}",
                            inner.as_display(),
                        ),
                    )
            }
            BlockExecutionError::CanonicalCommit { inner } => {
                __formatter
                    .write_fmt(
                        format_args!(
                            "transaction error on commit: {0}",
                            inner.as_display(),
                        ),
                    )
            }
            BlockExecutionError::AppendChainDoesntConnect {
                chain_tip,
                other_chain_fork,
            } => {
                __formatter
                    .write_fmt(
                        format_args!(
                            "appending chain on fork (other_chain_fork:?) is not possible as the tip is {0:?}",
                            chain_tip,
                        ),
                    )
            }
            BlockExecutionError::LatestBlock(_0) => {
                ::core::fmt::Display::fmt(_0, __formatter)
            }
            BlockExecutionError::Other(_0) => ::core::fmt::Display::fmt(_0, __formatter),
        }
    }
}
#[allow(unused_qualifications)]
impl ::core::convert::From<BlockValidationError> for BlockExecutionError {
    #[allow(deprecated)]
    fn from(source: BlockValidationError) -> Self {
        BlockExecutionError::Validation {
            0: source,
        }
    }
}
#[allow(unused_qualifications)]
impl ::core::convert::From<PruneSegmentError> for BlockExecutionError {
    #[allow(deprecated)]
    fn from(source: PruneSegmentError) -> Self {
        BlockExecutionError::Pruning {
            0: source,
        }
    }
}
#[allow(unused_qualifications)]
impl ::core::convert::From<ConsensusError> for BlockExecutionError {
    #[allow(deprecated)]
    fn from(source: ConsensusError) -> Self {
        BlockExecutionError::Consensus {
            0: source,
        }
    }
}
#[allow(unused_qualifications)]
impl ::core::convert::From<ProviderError> for BlockExecutionError {
    #[allow(deprecated)]
    fn from(source: ProviderError) -> Self {
        BlockExecutionError::LatestBlock {
            0: source,
        }
    }
}
#[automatically_derived]
impl ::core::fmt::Debug for BlockExecutionError {
    #[inline]
    fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
        match self {
            BlockExecutionError::Validation(__self_0) => {
                ::core::fmt::Formatter::debug_tuple_field1_finish(
                    f,
                    "Validation",
                    &__self_0,
                )
            }
            BlockExecutionError::Pruning(__self_0) => {
                ::core::fmt::Formatter::debug_tuple_field1_finish(
                    f,
                    "Pruning",
                    &__self_0,
                )
            }
            BlockExecutionError::Consensus(__self_0) => {
                ::core::fmt::Formatter::debug_tuple_field1_finish(
                    f,
                    "Consensus",
                    &__self_0,
                )
            }
            BlockExecutionError::CanonicalRevert { inner: __self_0 } => {
                ::core::fmt::Formatter::debug_struct_field1_finish(
                    f,
                    "CanonicalRevert",
                    "inner",
                    &__self_0,
                )
            }
            BlockExecutionError::CanonicalCommit { inner: __self_0 } => {
                ::core::fmt::Formatter::debug_struct_field1_finish(
                    f,
                    "CanonicalCommit",
                    "inner",
                    &__self_0,
                )
            }
            BlockExecutionError::AppendChainDoesntConnect {
                chain_tip: __self_0,
                other_chain_fork: __self_1,
            } => {
                ::core::fmt::Formatter::debug_struct_field2_finish(
                    f,
                    "AppendChainDoesntConnect",
                    "chain_tip",
                    __self_0,
                    "other_chain_fork",
                    &__self_1,
                )
            }
            BlockExecutionError::LatestBlock(__self_0) => {
                ::core::fmt::Formatter::debug_tuple_field1_finish(
                    f,
                    "LatestBlock",
                    &__self_0,
                )
            }
            BlockExecutionError::Other(__self_0) => {
                ::core::fmt::Formatter::debug_tuple_field1_finish(f, "Other", &__self_0)
            }
        }
    }
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
        match self {
            Self::CanonicalCommit { .. } | Self::CanonicalRevert { .. } => true,
            _ => false,
        }
    }
    /// Returns `true` if the error is a state root error.
    pub const fn is_state_root_error(&self) -> bool {
        match self {
            Self::Validation(BlockValidationError::StateRoot(_)) => true,
            _ => false,
        }
    }
}

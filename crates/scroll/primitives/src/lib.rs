//! Primitive types for the Scroll extension of `Reth`.

#![warn(unused_crate_dependencies)]

pub use execution_context::ScrollPostExecutionContext;
mod execution_context;

pub use account_extension::AccountExtension;
mod account_extension;

pub use l1_transaction::{
    ScrollL1MessageTransactionFields, TxL1Message, L1_MESSAGE_TRANSACTION_TYPE,
};
pub mod l1_transaction;

/// Poseidon hashing primitives.
pub mod poseidon;

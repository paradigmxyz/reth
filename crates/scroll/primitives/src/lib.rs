//! Primitive types for the Scroll extension of `Reth`.

#![warn(unused_crate_dependencies)]
#![cfg_attr(not(feature = "std"), no_std)]

pub use l1_transaction::{
    ScrollL1MessageTransactionFields, TxL1Message, L1_MESSAGE_TRANSACTION_TYPE,
};
pub mod l1_transaction;

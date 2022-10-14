#![warn(missing_debug_implementations, missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Commonly used types in reth.

mod account;
mod block;
mod chain;
mod forkid;
mod header;
mod jsonu256;
mod log;
mod receipt;
mod transaction;

pub use account::Account;
pub use block::{Block, BlockLocked};
pub use chain::Chain;
pub use forkid::{ForkFilter, ForkHash, ForkId};
pub use header::{Header, HeaderLocked};
pub use jsonu256::JsonU256;
pub use log::Log;
pub use receipt::Receipt;
pub use transaction::{AccessList, AccessListItem, Transaction, TransactionSigned, TxType};

/// Block Number is height of chain
pub type BlockNumber = u64;
/// Ethereum address
pub type Address = H160;
/// BlockId is Keccak hash of the header
pub type BlockID = H256;
/// TxHash is Kecack hash of rlp encoded signed transaction
pub type TxHash = H256;

/// Storage Key
pub type StorageKey = H256;

/// Storage value
pub type StorageValue = H256;

// NOTE: There is a benefit of using wrapped Bytes as it gives us serde and debug
pub use ethers_core::{
    types as rpc,
    types::{Bloom, Bytes, H160, H256, H512, H64, U256, U64},
};

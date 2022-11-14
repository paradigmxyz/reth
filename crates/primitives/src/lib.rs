#![warn(missing_debug_implementations, missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Commonly used types in reth.

mod account;
mod block;
pub mod bloom;
mod chain;
mod error;
mod header;
mod hex_bytes;
mod integer_list;
mod jsonu256;
mod log;
mod receipt;
mod storage;
mod transaction;

pub use account::Account;
pub use block::{Block, BlockLocked};
pub use chain::Chain;
pub use header::{Header, SealedHeader};
pub use hex_bytes::Bytes;
pub use integer_list::IntegerList;
pub use jsonu256::JsonU256;
pub use log::Log;
pub use receipt::Receipt;
pub use storage::StorageEntry;
pub use transaction::{
    AccessList, AccessListItem, Signature, Transaction, TransactionKind, TransactionSigned,
    TransactionSignedEcRecovered, TxType,
};

/// Block hash.
pub type BlockHash = H256;
/// Block Number is height of chain
pub type BlockNumber = u64;
/// Ethereum address
pub type Address = H160;
/// BlockId is Keccak hash of the header
pub type BlockID = H256;
/// TxHash is Kecack hash of rlp encoded signed transaction
pub type TxHash = H256;
/// TxNumber is sequence number of all existing transactions
pub type TxNumber = u64;
/// Chain identifier type, introduced in EIP-155
pub type ChainId = u64;
/// Storage Key
pub type StorageKey = H256;
/// Storage value
pub type StorageValue = U256;

pub use ethbloom::Bloom;
pub use ethers_core::{
    types as rpc,
    types::{BigEndianHash, H128, H160, H256, H512, H64, U128, U256, U64},
};

#[doc(hidden)]
mod __reexport {
    pub use hex;
    pub use hex_literal;
    pub use tiny_keccak;
}

// Useful reexports
pub use __reexport::*;

/// Returns the keccak256 hash for the given data.
pub fn keccak256(data: impl AsRef<[u8]>) -> H256 {
    use tiny_keccak::{Hasher, Keccak};
    let mut keccak = Keccak::v256();
    let mut output = [0; 32];
    keccak.update(data.as_ref());
    keccak.finalize(&mut output);
    output.into()
}

use hex_literal::hex;

/// Keccak256 over empty array.
pub const KECCAK_EMPTY: H256 =
    H256(hex!("c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"));

/// Ommer root of empty list.
pub const EMPTY_OMMER_ROOT: H256 =
    H256(hex!("1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347"));

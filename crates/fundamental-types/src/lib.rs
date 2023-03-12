#![warn(missing_docs, unreachable_pub, unused_crate_dependencies)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Fundamental types shared by [reth](https://github.com/paradigmxyz/reth) [revm](https://github.com/bluealloy/revm) and [ethers](https://github.com/gakonst/ethers-rs).

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod bits;

pub use bits::{B160, B256};

/// Address type is first 20 bytes of hash of ethereum account
pub type Address = B160;
/// Hash, in Ethereum usually kecack256.
pub type Hash = B256;

// ruint reexports
pub use ruint::{
    self,
    aliases::{B128 as H128, B64 as H64, U128, U256, U64},
    uint,
};

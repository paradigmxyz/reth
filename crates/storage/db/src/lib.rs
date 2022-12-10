//! Rust database abstraction and concrete database implementations.

#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

/// Abstracted part of database, containing traits for transactions and cursors.
pub mod abstraction;

mod implementation;
pub mod tables;
mod utils;

#[cfg(feature = "mdbx")]
/// Bindings for [MDBX](https://libmdbx.dqdkfa.ru/).
pub mod mdbx {
    pub use crate::implementation::mdbx::*;
    pub use reth_libmdbx::*;
}

pub use abstraction::*;
pub use reth_interfaces::db::Error;
pub use tables::*;

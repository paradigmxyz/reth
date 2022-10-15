#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! RLPx ECIES framed transport protocol.

pub mod algorithm;
pub mod mac;
pub mod proto;
pub mod util;

mod error;
pub use error::ECIESError;

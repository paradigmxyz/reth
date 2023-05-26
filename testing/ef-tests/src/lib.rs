#![warn(missing_debug_implementations, missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Abstractions and runners for EF tests.

pub mod case;
pub mod result;
pub mod suite;

pub mod assert;
pub mod cases;
pub mod models;

pub use case::{Case, Cases};
pub use result::{CaseResult, Error};
pub use suite::Suite;

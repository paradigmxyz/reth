#![warn(missing_docs, unreachable_pub, unused_crate_dependencies)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! revm utils and implementations specific to reth.

pub mod config;

/// Contains glue code for integrating reth database into revm's [Database](revm::Database).
pub mod database;

/// Helpers for configuring revm [Env](revm::primitives::Env)
pub mod env;

/// Helpers for type compatibility between reth and revm types
mod compat;
pub use compat::*;

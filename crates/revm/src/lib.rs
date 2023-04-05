#![warn(missing_docs, unreachable_pub, unused_crate_dependencies)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! revm utils and implementations specific to reth.

/// Contains glue code for integrating reth database into revm's [Database](revm::Database).
pub mod database;

/// revm implementation of reth block and transaction executors.
pub mod executor;
mod factory;

/// revm executor factory.
pub use factory::Factory;

/// reexport for convenience
pub use reth_revm_inspectors::*;
/// reexport for convenience
pub use reth_revm_primitives::*;

/// Re-export everything
pub use revm;

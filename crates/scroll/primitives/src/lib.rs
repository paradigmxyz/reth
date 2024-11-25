//! Primitive types for the Scroll extension of `Reth`.

#![warn(unused_crate_dependencies)]

pub use execution_context::ScrollPostExecutionContext;
mod execution_context;

pub use account_extension::AccountExtension;
mod account_extension;

pub use poseidon::{hash_code, POSEIDON_EMPTY};
mod poseidon;

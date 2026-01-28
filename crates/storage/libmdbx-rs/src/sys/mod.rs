//! System-level environment types and configuration.
//!
//! This module contains types for environment configuration that are not
//! commonly needed for basic usage.
//!
//! # Types
//!
//! - [`EnvironmentKind`] - Environment implementation variant (mmap mode)
//! - [`PageSize`] - Database page size configuration
//! - [`HandleSlowReadersCallback`] - Callback for handling slow readers
//! - [`HandleSlowReadersReturnCode`] - Return codes for slow reader callbacks

mod environment;
#[cfg(feature = "read-tx-timeouts")]
pub(crate) use environment::read_transactions;
pub(crate) use environment::EnvPtr;
pub use environment::{
    Environment, EnvironmentBuilder, EnvironmentKind, Geometry, HandleSlowReadersCallback,
    HandleSlowReadersReturnCode, Info, PageSize, Stat,
};

pub(crate) mod txn_manager;

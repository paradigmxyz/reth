#![warn(missing_debug_implementations, missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! reth's transaction pool implementation

pub mod error;

mod chain;
mod validate;

pub use chain::ChainInfo;
use std::sync::Arc;

mod config;
use crate::traits::TransactionPool;
pub use config::PoolConfig;

mod traits;

/// A generic `TransactionPool` implementation.
pub struct Pool<ChainApi, Transaction> {
    // TODO rm later
    _marker: std::marker::PhantomData<(ChainApi, Transaction)>,
}

// === impl Pool ===

impl<ChainApi, Transaction> Pool<ChainApi, Transaction>
where
    ChainApi: ChainInfo,
{
    /// Creates a new `Pool` with the given config and chain api
    pub fn new(config: PoolConfig, api: Arc<ChainApi>) -> Self {
        unimplemented!()
    }
}

/// implements the `TransactionPool` interface for the `Poll`.
impl<ChainApi, Transaction> TransactionPool for Pool<ChainApi, Transaction>
where
    ChainApi: ChainInfo,
    // TODO this could be unified by moving it ChaiApi trait
    Transaction: Send + Sync,
{
}

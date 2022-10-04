#![warn(missing_debug_implementations, missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Reth's transaction pool implementation

pub mod error;

mod client;
mod validate;

pub use client::PoolClient;
use parking_lot::RwLock;
use std::sync::Arc;

mod config;
pub use config::PoolConfig;

mod ordering;

pub mod pool;

mod traits;
pub use crate::{
    ordering::TransactionOrdering, traits::TransactionPool, validate::TransactionValidator,
};

/// A generic, customizable `TransactionPool` implementation.
// TODO: This is a more feature rich pool, any additional features should go here, like metrics,
// etc...
pub struct Pool<PoolApi: PoolClient, Ordering: TransactionOrdering> {
    /// The actual transaction pool where transactions are handled.
    inner: Arc<pool::PoolInner<PoolApi, Ordering>>,
    /// Chain/Storage access
    client: Arc<PoolApi>,
    // TODO how to revalidate
    // TODO provide a way to add listeners for ready transactions
}

// === impl Pool ===

impl<PoolApi, Ordering> Pool<PoolApi, Ordering>
where
    PoolApi: PoolClient,
    Ordering: TransactionOrdering,
{
    /// Creates a new `Pool` with the given config and chain api
    pub fn new(config: PoolConfig, api: Arc<PoolApi>) -> Self {
        unimplemented!()
    }
}

/// implements the `TransactionPool` interface for the `Poll`.
impl<PoolApi, Ordering> TransactionPool for Pool<PoolApi, Ordering>
where
    PoolApi: PoolClient,
    Ordering: TransactionOrdering,
{
}

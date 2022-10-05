#![warn(missing_docs)] // unreachable_pub, missing_debug_implementations
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Reth's transaction pool implementation

use reth_primitives::BlockId;
use std::sync::Arc;

mod client;
mod config;
pub mod error;
mod ordering;
pub mod pool;
mod traits;
mod validate;

pub use crate::{
    client::PoolClient,
    config::PoolConfig,
    ordering::TransactionOrdering,
    pool::BasicPool,
    traits::{PoolTransaction, TransactionPool},
    validate::TransactionValidator,
};

/// A generic, customizable `TransactionPool` implementation.
pub struct Pool<P: PoolClient, T: TransactionOrdering> {
    /// The actual transaction pool where transactions are handled.
    pool: BasicPool<P, T>,
    /// Chain/Storage access
    client: Arc<P>,
}

// === impl Pool ===

impl<P, T> Pool<P, T>
    where
        P: PoolClient,
        T: TransactionOrdering<Transaction = <P as TransactionValidator>::Transaction>,
{
    /// Creates a new `Pool` with the given config and client and ordering.
    pub fn new(client: Arc<P>, ordering: Arc<T>, config: PoolConfig) -> Self {
        let pool = BasicPool::new(Arc::clone(&client), ordering, config);
        Self { pool, client }
    }
}

/// implements the `TransactionPool` interface for the `Poll`.
impl<P, T> TransactionPool for Pool<P, T>
where
    P: PoolClient,
    T: TransactionOrdering<Transaction = <P as TransactionValidator>::Transaction>,
{
}

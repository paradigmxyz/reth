#![warn(missing_docs)] // unreachable_pub, missing_debug_implementations
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Reth's transaction pool implementation

use futures::channel::mpsc::Receiver;
use reth_primitives::{BlockId, U64};
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
    traits::{ChainEvent, PoolTransaction, ReadyTransactions, TransactionPool},
    validate::{TransactionValidationOutcome, TransactionValidator},
};
use crate::{error::PoolResult, traits::HashFor, validate::ValidPoolTransaction};

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

/// implements the `TransactionPool` interface for various transaction pool API consumers.
#[async_trait::async_trait]
impl<P, T> TransactionPool for Pool<P, T>
where
    P: PoolClient,
    T: TransactionOrdering<Transaction = <P as TransactionValidator>::Transaction>,
{
    type Transaction = T::Transaction;

    async fn on_chain_event(&self, _event: ChainEvent) {
        // TODO perform maintenance: update pool accordingly
        todo!()
    }

    async fn add_transaction(
        &self,
        block_id: BlockId,
        transaction: Self::Transaction,
    ) -> PoolResult<HashFor<Self>> {
        self.pool.clone().add_transaction(&block_id, transaction).await
    }

    async fn add_transactions(
        &self,
        block_id: BlockId,
        transactions: Vec<Self::Transaction>,
    ) -> PoolResult<Vec<PoolResult<HashFor<Self>>>> {
        self.pool.clone().add_transactions(&block_id, transactions).await
    }

    fn ready_transactions_listener(&self) -> Receiver<HashFor<Self>> {
        self.pool.ready_transactions_listener()
    }

    fn ready_transactions(
        &self,
    ) -> Box<dyn ReadyTransactions<Item = Arc<ValidPoolTransaction<Self::Transaction>>>> {
        Box::new(self.pool.inner().ready_transactions())
    }

    fn remove_invalid(
        &self,
        _tx_hashes: &[HashFor<Self>],
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        todo!()
    }
}

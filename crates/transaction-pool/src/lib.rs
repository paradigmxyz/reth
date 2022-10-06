#![warn(missing_docs)] // unreachable_pub, missing_debug_implementations
#![allow(unused)] // TODO(mattsse) remove after progress was made
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Reth's transaction pool implementation

use futures::channel::mpsc::Receiver;
use parking_lot::Mutex;
use reth_primitives::{BlockID, TxHash, U256, U64};
use std::sync::Arc;

mod client;
mod config;
pub mod error;
mod identifier;
mod ordering;
pub mod pool;
mod traits;
mod validate;

pub use crate::{
    client::PoolClient,
    config::PoolConfig,
    ordering::TransactionOrdering,
    pool::BasicPool,
    traits::{BestTransactions, NewBlockEvent, PoolTransaction, TransactionPool},
    validate::{TransactionValidationOutcome, TransactionValidator},
};
use crate::{error::PoolResult, validate::ValidPoolTransaction};

/// A generic, customizable `TransactionPool` implementation.
pub struct Pool<P: PoolClient, T: TransactionOrdering> {
    /// The actual transaction pool where transactions and subscriptions are handled.
    pool: BasicPool<P, T>,
    /// Tracks status updates linked to chain events.
    update_status: Arc<Mutex<UpdateStatus>>,
    /// Chain/Storage access.
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
        Self { pool, update_status: Arc::new(Default::default()), client }
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

    async fn on_new_block(&self, _event: NewBlockEvent) {
        // TODO perform maintenance: update pool accordingly
        todo!()
    }

    async fn add_transaction(
        &self,
        block_id: BlockID,
        transaction: Self::Transaction,
    ) -> PoolResult<TxHash> {
        self.pool.clone().add_transaction(&block_id, transaction).await
    }

    async fn add_transactions(
        &self,
        block_id: BlockID,
        transactions: Vec<Self::Transaction>,
    ) -> PoolResult<Vec<PoolResult<TxHash>>> {
        self.pool.clone().add_transactions(&block_id, transactions).await
    }

    fn ready_transactions_listener(&self) -> Receiver<TxHash> {
        self.pool.ready_transactions_listener()
    }

    fn best_transactions(
        &self,
    ) -> Box<dyn BestTransactions<Item = Arc<ValidPoolTransaction<Self::Transaction>>>> {
        Box::new(self.pool.inner().ready_transactions())
    }

    fn remove_invalid(
        &self,
        _tx_hashes: &[TxHash],
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        todo!()
    }
}

/// Tracks the current update status of the pool.
#[derive(Debug, Clone, Default)]
struct UpdateStatus {
    /// Block number when the pool was last updated.
    updated_at: U64,
    /// Current base fee that needs to be enforced
    base_fee: U256,
}

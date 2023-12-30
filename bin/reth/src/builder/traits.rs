//! Traits for the builder process.

use reth_transaction_pool::TransactionPool;

/// A trait for building a transaction pool.
#[async_trait::async_trait]
pub trait PoolBuilder {
    /// The transaction pool to use.
    type Pool: TransactionPool;

    /// Builds the transaction pool.
    ///
    /// Note: Implementors are responsible spawning any background tasks required by the pool, e,g, [maintain_transaction_pool](reth_transaction_pool::maintain::maintain_transaction_pool).
    async fn build_pool(self) -> eyre::Result<Self::Pool>;
}
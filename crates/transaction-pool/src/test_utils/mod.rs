//! Internal helpers for testing.
#![allow(missing_docs, unused, missing_debug_implementations, unreachable_pub)]

mod mock;
mod pool;

use crate::{
    Pool, PoolTransaction, TransactionOrigin, TransactionValidationOutcome, TransactionValidator,
};
use async_trait::async_trait;
pub use mock::*;
use std::{marker::PhantomData, sync::Arc};

/// A [Pool] used for testing
pub type TestPool = Pool<NoopTransactionValidator<MockTransaction>, MockOrdering>;

/// Returns a new [Pool] used for testing purposes
pub fn testing_pool() -> TestPool {
    Pool::new(
        Arc::new(NoopTransactionValidator::default()),
        Arc::new(MockOrdering::default()),
        Default::default(),
    )
}

// A [`TransactionValidator`] that does nothing.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct NoopTransactionValidator<T>(PhantomData<T>);

#[async_trait::async_trait]
impl<T: PoolTransaction> TransactionValidator for NoopTransactionValidator<T> {
    type Transaction = T;

    async fn validate_transaction(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> TransactionValidationOutcome<Self::Transaction> {
        TransactionValidationOutcome::Valid {
            balance: Default::default(),
            state_nonce: 0,
            transaction,
        }
    }
}

impl<T> Default for NoopTransactionValidator<T> {
    fn default() -> Self {
        NoopTransactionValidator(PhantomData::default())
    }
}

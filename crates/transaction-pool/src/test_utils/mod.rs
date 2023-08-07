//! Internal helpers for testing.
#![allow(missing_docs, unused, missing_debug_implementations, unreachable_pub)]

mod mock;
mod pool;

use crate::{
    noop::NoopTransactionValidator, Pool, PoolTransaction, TransactionOrigin,
    TransactionValidationOutcome, TransactionValidator,
};
use async_trait::async_trait;
pub use mock::*;
use std::{marker::PhantomData, sync::Arc};

/// A [Pool] used for testing
pub type TestPool = Pool<NoopTransactionValidator<MockTransaction>, MockOrdering>;

/// Returns a new [Pool] used for testing purposes
pub fn testing_pool() -> TestPool {
    Pool::new(NoopTransactionValidator::default(), MockOrdering::default(), Default::default())
}

//! Internal helpers for testing.
#![allow(missing_docs, unused, missing_debug_implementations, unreachable_pub)]

mod mock;
mod pool;

use crate::{
    noop::MockTransactionValidator, Pool, PoolTransaction, TransactionOrigin,
    TransactionValidationOutcome, TransactionValidator,
};
use async_trait::async_trait;
pub use mock::*;
use std::{marker::PhantomData, sync::Arc};

/// A [Pool] used for testing
pub type TestPool = Pool<MockTransactionValidator<MockTransaction>, MockOrdering>;

/// Returns a new [Pool] used for testing purposes
pub fn testing_pool() -> TestPool {
    testing_pool_with_validator(MockTransactionValidator::default())
}
/// Returns a new [Pool] used for testing purposes
pub fn testing_pool_with_validator(
    validator: MockTransactionValidator<MockTransaction>,
) -> TestPool {
    Pool::new(validator, MockOrdering::default(), Default::default())
}

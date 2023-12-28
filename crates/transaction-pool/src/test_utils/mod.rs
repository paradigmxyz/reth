//! Internal helpers for testing.

#![allow(missing_docs, missing_debug_implementations)]

use crate::{blobstore::InMemoryBlobStore, noop::MockTransactionValidator, Pool};

mod gen;
pub use gen::*;

mod mock;
pub use mock::*;

mod pool;

/// A [Pool] used for testing
pub type TestPool =
    Pool<MockTransactionValidator<MockTransaction>, MockOrdering, InMemoryBlobStore>;

/// Returns a new [Pool] used for testing purposes
pub fn testing_pool() -> TestPool {
    testing_pool_with_validator(MockTransactionValidator::default())
}

/// Returns a new [Pool] used for testing purposes
pub fn testing_pool_with_validator(
    validator: MockTransactionValidator<MockTransaction>,
) -> TestPool {
    Pool::new(validator, MockOrdering::default(), InMemoryBlobStore::default(), Default::default())
}

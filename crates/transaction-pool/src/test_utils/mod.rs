//! Internal helpers for testing.

use crate::{blobstore::InMemoryBlobStore, noop::MockTransactionValidator, Pool, PoolConfig};
use std::ops::{Deref, DerefMut};

mod gen;
pub use gen::*;

mod mock;
pub use mock::*;

mod pool;

/// Structure encapsulating a [Pool] used for testing
#[derive(Debug)]
pub struct TestPool(
    Pool<MockTransactionValidator<MockTransaction>, MockOrdering, InMemoryBlobStore>,
);

impl Default for TestPool {
    fn default() -> Self {
        Self(Pool::new(
            MockTransactionValidator::default(),
            MockOrdering::default(),
            InMemoryBlobStore::default(),
            Default::default(),
        ))
    }
}

impl TestPool {
    /// Returns a new [TestPool] with a custom validator used for testing purposes
    pub fn with_validator(self, validator: MockTransactionValidator<MockTransaction>) -> Self {
        Self(Pool::new(
            validator,
            MockOrdering::default(),
            self.pool.blob_store().clone(),
            self.pool.config().clone(),
        ))
    }

    /// Returns a new [TestPool] with a custom ordering used for testing purposes
    pub fn with_ordering(self, ordering: MockOrdering) -> Self {
        Self(Pool::new(
            self.pool.validator().clone(),
            ordering,
            self.pool.blob_store().clone(),
            self.pool.config().clone(),
        ))
    }

    /// Returns a new [TestPool] with a custom blob store used for testing purposes
    pub fn with_blob_store(self, blob_store: InMemoryBlobStore) -> Self {
        Self(Pool::new(
            self.pool.validator().clone(),
            MockOrdering::default(),
            blob_store,
            self.pool.config().clone(),
        ))
    }

    /// Returns a new [TestPool] with a custom configuration used for testing purposes
    pub fn with_config(self, config: PoolConfig) -> Self {
        Self(Pool::new(
            self.pool.validator().clone(),
            MockOrdering::default(),
            self.pool.blob_store().clone(),
            config,
        ))
    }
}

impl Deref for TestPool {
    type Target = Pool<MockTransactionValidator<MockTransaction>, MockOrdering, InMemoryBlobStore>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for TestPool {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

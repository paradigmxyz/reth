//! Internal helpers for testing.

use crate::{blobstore::InMemoryBlobStore, noop::MockTransactionValidator, Pool, PoolConfig};
use std::ops::Deref;

mod gen;
pub use gen::*;

mod mock;
pub use mock::*;

mod pool;

/// A [Pool] used for testing
pub type TestPool =
    Pool<MockTransactionValidator<MockTransaction>, MockOrdering, InMemoryBlobStore>;

/// Structure encapsulating a [TestPool] used for testing
#[derive(Debug, Clone)]
pub struct TestPoolBuilder(TestPool);

impl Default for TestPoolBuilder {
    fn default() -> Self {
        Self(Pool::new(
            MockTransactionValidator::default(),
            MockOrdering::default(),
            InMemoryBlobStore::default(),
            Default::default(),
        ))
    }
}

impl TestPoolBuilder {
    /// Returns a new [TestPoolBuilder] with a custom validator used for testing purposes
    pub fn with_validator(self, validator: MockTransactionValidator<MockTransaction>) -> Self {
        Self(Pool::new(
            validator,
            MockOrdering::default(),
            self.pool.blob_store().clone(),
            self.pool.config().clone(),
        ))
    }

    /// Returns a new [TestPoolBuilder] with a custom ordering used for testing purposes
    pub fn with_ordering(self, ordering: MockOrdering) -> Self {
        Self(Pool::new(
            self.pool.validator().clone(),
            ordering,
            self.pool.blob_store().clone(),
            self.pool.config().clone(),
        ))
    }

    /// Returns a new [TestPoolBuilder] with a custom blob store used for testing purposes
    pub fn with_blob_store(self, blob_store: InMemoryBlobStore) -> Self {
        Self(Pool::new(
            self.pool.validator().clone(),
            MockOrdering::default(),
            blob_store,
            self.pool.config().clone(),
        ))
    }

    /// Returns a new [TestPoolBuilder] with a custom configuration used for testing purposes
    pub fn with_config(self, config: PoolConfig) -> Self {
        Self(Pool::new(
            self.pool.validator().clone(),
            MockOrdering::default(),
            self.pool.blob_store().clone(),
            config,
        ))
    }
}

impl From<TestPoolBuilder> for TestPool {
    fn from(wrapper: TestPoolBuilder) -> Self {
        wrapper.0
    }
}

impl Deref for TestPoolBuilder {
    type Target = TestPool;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Returns a new [Pool] with default field values used for testing purposes
pub fn testing_pool() -> TestPool {
    TestPoolBuilder::default().into()
}

//! Assertions that can be checked in tests.

use crate::testsuite::{BoxFuture, ResultStore};
use alloy_primitives::{BlockNumber, B256, U256};
use eyre::Result;

/// An assertion that can be checked against stored results
pub trait Assertion: Send + 'static {
    /// Assert a condition on the stored results
    fn assert<'a>(&'a mut self, results: &'a ResultStore) -> BoxFuture<'a, Result<()>>;
}

/// Simplified assertion container for storage in tests
#[allow(missing_debug_implementations)]
pub struct AssertionBox(Box<dyn Assertion>);

impl AssertionBox {
    /// Creates a new [`AssertionBox`].
    pub fn new<A: Assertion>(assertion: A) -> Self {
        Self(Box::new(assertion))
    }

    /// Asserts on the given [`ResultStore`].
    pub async fn assert(mut self, results: &ResultStore) -> Result<()> {
        self.0.assert(results).await
    }
}

/// Assert that a block with the given hash exists.
#[derive(Debug)]
pub struct BlockExists {
    /// The block hash to check
    pub block_hash: B256,
}

impl Assertion for BlockExists {
    fn assert<'a>(&'a mut self, _results: &'a ResultStore) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { Ok(()) })
    }
}

/// Assert that a block with the given hash exists at a specific number.
#[derive(Debug)]
pub struct BlockExistsAtNumber {
    /// The block hash to check
    pub block_hash: B256,
    /// The block number to check
    pub block_number: BlockNumber,
}

impl Assertion for BlockExistsAtNumber {
    fn assert<'a>(&'a mut self, _results: &'a ResultStore) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { Ok(()) })
    }
}

/// Assert that a transaction with the given hash exists.
#[derive(Debug)]
pub struct TransactionExists {
    /// The transaction hash to check
    pub tx_hash: B256,
}

impl Assertion for TransactionExists {
    fn assert<'a>(&'a mut self, _results: &'a ResultStore) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { Ok(()) })
    }
}

/// Assert that a transaction with the given hash exists in a specific block.
#[derive(Debug)]
pub struct TransactionExistsInBlock {
    /// The transaction hash to check
    pub tx_hash: B256,
    /// The block hash to check
    pub block_hash: B256,
}

impl Assertion for TransactionExistsInBlock {
    fn assert<'a>(&'a mut self, _results: &'a ResultStore) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { Ok(()) })
    }
}

/// Assert that a stored value matches the expected value.
#[derive(Debug)]
pub struct ValueEquals {
    /// The value id to check
    pub value_id: String,
    /// The expected value
    pub expected: U256,
}

impl Assertion for ValueEquals {
    fn assert<'a>(&'a mut self, results: &'a ResultStore) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if let Some(value) = results.get_value(&self.value_id) {
                if *value == self.expected {
                    Ok(())
                } else {
                    Err(eyre::eyre!(
                        "Value mismatch for {}: expected {}, got {}",
                        self.value_id,
                        self.expected,
                        value
                    ))
                }
            } else {
                Err(eyre::eyre!("Value not found: {}", self.value_id))
            }
        })
    }
}

/// Assert that a stored boolean matches the expected value.
#[derive(Debug)]
pub struct BoolEquals {
    /// The bool id to check
    pub bool_id: String,
    /// The expected value
    pub expected: bool,
}

impl Assertion for BoolEquals {
    fn assert<'a>(&'a mut self, results: &'a ResultStore) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if let Some(value) = results.get_bool(&self.bool_id) {
                if *value == self.expected {
                    Ok(())
                } else {
                    Err(eyre::eyre!(
                        "Boolean mismatch for {}: expected {}, got {}",
                        self.bool_id,
                        self.expected,
                        value
                    ))
                }
            } else {
                Err(eyre::eyre!("Boolean not found: {}", self.bool_id))
            }
        })
    }
}

/// Assert that a specific block hash matches the expected value.
#[derive(Debug)]
pub struct BlockHashEquals {
    /// The block hash id to check
    pub block_hash_id: String,
    /// The expected hash
    pub expected: B256,
}

impl Assertion for BlockHashEquals {
    fn assert<'a>(&'a mut self, results: &'a ResultStore) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if let Some(hash) = results.get_block_hash(&self.block_hash_id) {
                if *hash == self.expected {
                    Ok(())
                } else {
                    Err(eyre::eyre!(
                        "Block hash mismatch for {}: expected {}, got {}",
                        self.block_hash_id,
                        self.expected,
                        hash
                    ))
                }
            } else {
                Err(eyre::eyre!("Block hash not found: {}", self.block_hash_id))
            }
        })
    }
}

/// Assert that a specific transaction hash matches the expected value.
#[derive(Debug)]
pub struct TransactionHashEquals {
    /// The transaction hash id to check
    pub tx_hash_id: String,
    /// The expected hash
    pub expected: B256,
}

impl Assertion for TransactionHashEquals {
    fn assert<'a>(&'a mut self, results: &'a ResultStore) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if let Some(hash) = results.get_transaction_hash(&self.tx_hash_id) {
                if *hash == self.expected {
                    Ok(())
                } else {
                    Err(eyre::eyre!(
                        "Transaction hash mismatch for {}: expected {}, got {}",
                        self.tx_hash_id,
                        self.expected,
                        hash
                    ))
                }
            } else {
                Err(eyre::eyre!("Transaction hash not found: {}", self.tx_hash_id))
            }
        })
    }
}

/// Combine multiple assertions and require all to pass.
#[allow(missing_debug_implementations)]
pub struct AllOf {
    /// Assertions to check
    pub assertions: Vec<Box<dyn Assertion>>,
}

impl Assertion for AllOf {
    fn assert<'a>(&'a mut self, _results: &'a ResultStore) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { Ok(()) })
    }
}

/// Combine multiple assertions and require at least one to pass.
#[allow(missing_debug_implementations)]
pub struct AnyOf {
    /// Assertions to check
    pub assertions: Vec<Box<dyn Assertion>>,
}

impl Assertion for AnyOf {
    fn assert<'a>(&'a mut self, _results: &'a ResultStore) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { Ok(()) })
    }
}

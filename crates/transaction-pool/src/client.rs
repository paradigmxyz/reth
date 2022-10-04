//! Provides access to the chain's storage

use crate::{traits, traits::PoolTransaction, validate::TransactionValidator};
use std::hash;

// TODO could just merge with `TransactionValidator` into a single trait
#[async_trait::async_trait]
pub trait PoolClient: Send + Sync + TransactionValidator {
    /// Error type that can be converted to the crate's internal Error.
    type Error: Into<crate::error::PoolError>;

    /// Transaction hash type.
    type Hash: hash::Hash + Eq + Send + Sync + Sync;

    /// Block hash type
    type BlockHash: hash::Hash + Eq + Send + Sync;

    // TODO add functions to fetch Block/Hashes etc...
}

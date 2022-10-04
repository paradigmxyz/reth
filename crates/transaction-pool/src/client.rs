//! Provides access to the chain's storage

use crate::{traits, traits::PoolTransaction, validate::TransactionValidator};
use std::hash;

/// The interface used to interact with the blockchain and access storage.
#[async_trait::async_trait]
pub trait PoolClient: Send + Sync + TransactionValidator {
    /// Error type that can be converted to the crate's internal Error.
    type Error: Into<crate::error::PoolError>;

    /// Block hash type
    type BlockHash: hash::Hash + Eq + Send + Sync;

    // TODO add functions to fetch Block/Hashes etc...
}

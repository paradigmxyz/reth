//! Provides access to the chain's storage

use crate::{error::PoolError, validate::TransactionValidator};

/// The interface used to interact with the blockchain and access storage.
#[async_trait::async_trait]
pub trait PoolClient: Send + Sync + TransactionValidator {
    /// Error type that can be converted to the crate's internal Error.
    type Error: Into<PoolError>;
}

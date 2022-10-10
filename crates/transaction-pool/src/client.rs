//! Provides access to the chain's storage

use crate::{
    error::{PoolError, PoolResult},
    validate::TransactionValidator,
};
use reth_primitives::{BlockID, U64};

/// The interface used to interact with the blockchain and access storage.
#[async_trait::async_trait]
pub trait PoolClient: Send + Sync + TransactionValidator {
    /// Error type that can be converted to the crate's internal Error.
    type Error: Into<PoolError>;

    /// Returns the block number for the given block identifier.
    fn convert_block_id(&self, block_id: &BlockID) -> PoolResult<Option<U64>>;

    /// Same as [`PoolClient::convert_block_id()`] but returns an error if no matching block number
    /// was found
    fn ensure_block_number(&self, block_id: &BlockID) -> PoolResult<U64> {
        self.convert_block_id(block_id)
            .and_then(|number| number.ok_or_else(|| PoolError::BlockNumberNotFound(*block_id)))
    }
}

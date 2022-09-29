//! Provides access to the chain's storage

// TODO probably merge with `Validator` trait? since the validator also needs chain access
#[async_trait::async_trait]
pub trait ChainInfo: Send + Sync {
    /// The error type that can be converted to the crate's internal Error
    type Error: Into<crate::error::Error>;

    // TODO add functions to fetch Block/Hashes etc...
}

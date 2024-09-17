use std::num::ParseIntError;

/// Error while parsing a `ReceiptsLogPruneConfig`
#[derive(thiserror::Error, Debug)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum ReceiptsLogError {
    /// The format of the filter is invalid.
    #[error("invalid filter format: {0}")]
    InvalidFilterFormat(String),
    /// Address is invalid.
    #[error("address is invalid: {0}")]
    InvalidAddress(String),
    /// The prune mode is not one of full, distance, before.
    #[error("prune mode is invalid: {0}")]
    InvalidPruneMode(String),
    /// The distance value supplied is invalid.
    #[error("distance is invalid: {0}")]
    InvalidDistance(ParseIntError),
    /// The block number supplied is invalid.
    #[error("block number is invalid: {0}")]
    InvalidBlockNumber(ParseIntError),
}

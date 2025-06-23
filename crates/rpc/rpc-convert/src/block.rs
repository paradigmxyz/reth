//! Conversion trait for block response to primitive block types.

use alloy_network::Network;
use std::convert::Infallible;

/// Trait for converting from alloy network block responses to primitive blocks.
pub trait TryFromBlockResponse<N: Network> {
    /// The error type returned when conversion fails.
    type Error: core::error::Error + Send + Sync + Unpin;

    /// Converts from a network block response to the primitive block type.
    fn from_block_response(block_response: N::BlockResponse) -> Result<Self, Self::Error>
    where
        Self: Sized;
}

impl<N: Network, T> TryFromBlockResponse<N> for alloy_consensus::Block<T>
where
    N::BlockResponse: Into<Self>,
{
    type Error = Infallible;

    fn from_block_response(block_response: N::BlockResponse) -> Result<Self, Self::Error> {
        Ok(block_response.into())
    }
}

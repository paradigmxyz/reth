//! Conversion traits for block responses to primitive block types.

use alloy_network::Network;
use std::convert::Infallible;

/// Trait for converting network block responses to primitive block types.
pub trait TryFromBlockResponse<N: Network> {
    /// The error type returned if the conversion fails.
    type Error: core::error::Error + Send + Sync + Unpin;

    /// Converts a network block response to a primitive block type.
    ///
    /// # Returns
    ///
    /// Returns `Ok(Self)` on successful conversion, or `Err(Self::Error)` if the conversion fails.
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Block, TxEnvelope};
    use alloy_network::Ethereum;
    use alloy_rpc_types_eth::BlockTransactions;

    #[test]
    fn test_try_from_block_response() {
        let rpc_block: alloy_rpc_types_eth::Block =
            alloy_rpc_types_eth::Block::new(Default::default(), BlockTransactions::Full(vec![]));
        let result =
            <Block<TxEnvelope> as TryFromBlockResponse<Ethereum>>::from_block_response(rpc_block);
        assert!(result.is_ok());
    }
}

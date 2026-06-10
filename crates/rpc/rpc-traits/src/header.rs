use alloy_consensus::Sealable;
use alloy_primitives::U256;
use reth_primitives_traits::SealedHeader;

/// Conversion trait for obtaining RPC header from a consensus header.
pub trait FromConsensusHeader<T> {
    /// Takes a consensus header and converts it into `self`.
    fn from_consensus_header(header: SealedHeader<T>, block_size: usize) -> Self;
}

impl<T: Sealable> FromConsensusHeader<T> for alloy_rpc_types_eth::Header<T> {
    fn from_consensus_header(header: SealedHeader<T>, block_size: usize) -> Self {
        Self::from_consensus(header.into(), None, Some(U256::from(block_size)))
    }
}

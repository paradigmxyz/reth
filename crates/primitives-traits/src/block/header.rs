//! Block header data primitive.

use core::fmt;

use alloy_primitives::{Address, BlockNumber, Bloom, Bytes, Sealable, B256, B64, U256};
use reth_codecs::Compact;

use crate::{InMemorySize, MaybeSerde};

/// Helper trait that unifies all behaviour required by block header to support full node
/// operations.
pub trait FullBlockHeader: BlockHeader + Compact {}

impl<T> FullBlockHeader for T where T: BlockHeader + Compact {}

/// Abstraction of a block header.
pub trait BlockHeader:
    Send
    + Sync
    + Unpin
    + Clone
    + Default
    + fmt::Debug
    + PartialEq
    + Eq
    + alloy_rlp::Encodable
    + alloy_rlp::Decodable
    + alloy_consensus::BlockHeader
    + Sealable
    + InMemorySize
    + MaybeSerde
{
}

impl<T> BlockHeader for T where
    T: Send
        + Sync
        + Unpin
        + Clone
        + Default
        + fmt::Debug
        + PartialEq
        + Eq
        + alloy_rlp::Encodable
        + alloy_rlp::Decodable
        + alloy_consensus::BlockHeader
        + Sealable
        + InMemorySize
        + MaybeSerde
{
}

/// Helper trait to implement [`BlockHeader`] functionality for all [`Block`](crate::Block) types.
pub trait Header {
    /// See [`alloy_consensus::BlockHeader`].
    fn parent_hash(&self) -> B256;
    /// See [`alloy_consensus::BlockHeader`].
    fn ommers_hash(&self) -> B256;
    /// See [`alloy_consensus::BlockHeader`].
    fn beneficiary(&self) -> Address;
    /// See [`alloy_consensus::BlockHeader`].
    fn state_root(&self) -> B256;
    /// See [`alloy_consensus::BlockHeader`].
    fn transactions_root(&self) -> B256;
    /// See [`alloy_consensus::BlockHeader`].
    fn receipts_root(&self) -> B256;
    /// See [`alloy_consensus::BlockHeader`].
    fn withdrawals_root(&self) -> Option<B256>;
    /// See [`alloy_consensus::BlockHeader`].
    fn logs_bloom(&self) -> Bloom;
    /// See [`alloy_consensus::BlockHeader`].
    fn difficulty(&self) -> U256;
    /// See [`alloy_consensus::BlockHeader`].
    fn number(&self) -> BlockNumber;
    /// See [`alloy_consensus::BlockHeader`].
    fn gas_limit(&self) -> u64;
    /// See [`alloy_consensus::BlockHeader`].
    fn gas_used(&self) -> u64;
    /// See [`alloy_consensus::BlockHeader`].
    fn timestamp(&self) -> u64;
    /// See [`alloy_consensus::BlockHeader`].
    fn mix_hash(&self) -> Option<B256>;
    /// See [`alloy_consensus::BlockHeader`].
    fn nonce(&self) -> Option<B64>;
    /// See [`alloy_consensus::BlockHeader`].
    fn base_fee_per_gas(&self) -> Option<u64>;
    /// See [`alloy_consensus::BlockHeader`].
    fn blob_gas_used(&self) -> Option<u64>;
    /// See [`alloy_consensus::BlockHeader`].
    fn excess_blob_gas(&self) -> Option<u64>;
    /// See [`alloy_consensus::BlockHeader`].
    fn parent_beacon_block_root(&self) -> Option<B256>;
    /// See [`alloy_consensus::BlockHeader`].
    fn requests_hash(&self) -> Option<B256>;
    /// See [`alloy_consensus::BlockHeader`].
    fn extra_data(&self) -> &Bytes;
    /// See [`alloy_consensus::BlockHeader`].
    fn next_block_excess_blob_gas(&self) -> Option<u64>;
    /// See [`alloy_consensus::BlockHeader`].
    fn next_block_blob_fee(&self) -> Option<u128>;
}

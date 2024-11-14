//! Block abstraction.

pub mod body;
pub mod header;

use alloc::{fmt, vec::Vec};

use alloy_consensus::BlockHeader as _;
use alloy_eips::eip7685::Requests;
use alloy_primitives::{Address, BlockNumber, Bloom, Bytes, B256, B64, U256};
use reth_codecs::Compact;

use crate::{BlockBody, BlockHeader, Body, FullBlockBody, FullBlockHeader, Header, InMemorySize};

/// Helper trait that unifies all behaviour required by block to support full node operations.
pub trait FullBlock:
    Block<Header: FullBlockHeader, Body: FullBlockBody>
    + Compact
    + alloy_rlp::Encodable
    + alloy_rlp::Decodable
{
}

impl<T> FullBlock for T where
    T: Block<Header: FullBlockHeader, Body: FullBlockBody>
        + Compact
        + alloy_rlp::Encodable
        + alloy_rlp::Decodable
{
}

/// Abstraction of block data type.
// todo: make sealable super-trait, depends on <https://github.com/paradigmxyz/reth/issues/11449>
// todo: make with senders extension trait, so block can be impl by block type already containing
// senders
pub trait Block:
    Send
    + Sync
    + Unpin
    + Clone
    + Default
    + fmt::Debug
    + PartialEq
    + Eq
    + serde::Serialize
    + for<'a> serde::Deserialize<'a>
    + Header
    + Body<
        Self::Header,
        <Self::Body as BlockBody>::Transaction,
        <Self::Body as BlockBody>::Withdrawals,
    > + InMemorySize
{
    /// Header part of the block.
    type Header: BlockHeader;

    /// The block's body contains the transactions in the block.
    type Body: BlockBody<Header = Self::Header>;

    /// Returns reference to block header.
    fn header(&self) -> &Self::Header;

    /// Returns reference to block body.
    fn body(&self) -> &Self::Body;
}

impl<T: Block> Header for T {
    #[inline]
    fn parent_hash(&self) -> B256 {
        self.header().parent_hash()
    }

    #[inline]
    fn ommers_hash(&self) -> B256 {
        self.header().ommers_hash()
    }

    #[inline]
    fn beneficiary(&self) -> Address {
        self.header().beneficiary()
    }

    #[inline]
    fn state_root(&self) -> B256 {
        self.header().state_root()
    }

    #[inline]
    fn transactions_root(&self) -> B256 {
        self.header().transactions_root()
    }

    #[inline]
    fn receipts_root(&self) -> B256 {
        self.header().receipts_root()
    }

    #[inline]
    fn withdrawals_root(&self) -> Option<B256> {
        self.header().withdrawals_root()
    }

    #[inline]
    fn logs_bloom(&self) -> Bloom {
        self.header().logs_bloom()
    }

    #[inline]
    fn difficulty(&self) -> U256 {
        self.header().difficulty()
    }

    #[inline]
    fn number(&self) -> BlockNumber {
        self.header().number()
    }

    #[inline]
    fn gas_limit(&self) -> u64 {
        self.header().gas_limit()
    }

    #[inline]
    fn gas_used(&self) -> u64 {
        self.header().gas_used()
    }

    #[inline]
    fn timestamp(&self) -> u64 {
        self.header().timestamp()
    }

    #[inline]
    fn mix_hash(&self) -> Option<B256> {
        self.header().mix_hash()
    }

    #[inline]
    fn nonce(&self) -> Option<B64> {
        self.header().nonce()
    }

    #[inline]
    fn base_fee_per_gas(&self) -> Option<u64> {
        self.header().base_fee_per_gas()
    }

    #[inline]
    fn blob_gas_used(&self) -> Option<u64> {
        self.header().blob_gas_used()
    }

    #[inline]
    fn excess_blob_gas(&self) -> Option<u64> {
        self.header().excess_blob_gas()
    }

    #[inline]
    fn parent_beacon_block_root(&self) -> Option<B256> {
        self.header().parent_beacon_block_root()
    }

    #[inline]
    fn requests_hash(&self) -> Option<B256> {
        self.header().requests_hash()
    }

    #[inline]
    fn extra_data(&self) -> &Bytes {
        self.header().extra_data()
    }

    #[inline]
    fn next_block_excess_blob_gas(&self) -> Option<u64> {
        self.header().next_block_excess_blob_gas()
    }

    #[inline]
    fn next_block_blob_fee(&self) -> Option<u128> {
        self.header().next_block_blob_fee()
    }
}

impl<T: Block>
    Body<T::Header, <T::Body as BlockBody>::Transaction, <T::Body as BlockBody>::Withdrawals>
    for T
{
    #[inline]
    fn transactions(&self) -> &[<T::Body as BlockBody>::Transaction] {
        self.body().transactions()
    }

    #[inline]
    fn withdrawals(&self) -> Option<&<T::Body as BlockBody>::Withdrawals> {
        self.body().withdrawals()
    }

    #[inline]
    fn ommers(&self) -> &[T::Header] {
        self.body().ommers()
    }

    #[inline]
    fn requests(&self) -> Option<&Requests> {
        self.body().requests()
    }

    #[inline]
    fn calculate_tx_root(&self) -> B256 {
        self.body().calculate_tx_root()
    }

    #[inline]
    fn calculate_ommers_root(&self) -> B256 {
        self.body().calculate_ommers_root()
    }

    #[inline]
    fn calculate_withdrawals_root(&self) -> Option<B256> {
        self.body().calculate_withdrawals_root()
    }

    #[inline]
    fn recover_signers(&self) -> Option<Vec<Address>> {
        self.body().recover_signers()
    }

    #[inline]
    fn blob_versioned_hashes(&self) -> Vec<&B256> {
        self.body().blob_versioned_hashes()
    }

    #[inline]
    fn blob_versioned_hashes_copied(&self) -> Vec<B256> {
        self.body().blob_versioned_hashes_copied()
    }
}

//! Block abstraction.

pub mod body;
pub mod header;

use core::fmt;

use alloy_consensus::BlockHeader as _;
use alloy_primitives::{Address, BlockNumber, Bloom, Bytes, B256, B64, U256};
use reth_codecs::Compact;

use crate::{BlockHeader, FullBlockBody, FullBlockHeader, Header, InMemorySize, MaybeSerde};

/// Helper trait that unifies all behaviour required by block to support full node operations.
pub trait FullBlock: Block<Header: Compact> {}

impl<T> FullBlock for T where T: Block<Header: FullBlockHeader> {}

/// Abstraction of block data type.
// todo: make sealable super-trait, depends on <https://github.com/paradigmxyz/reth/issues/11449>
// todo: make with senders extension trait, so block can be impl by block type already containing
// senders
#[auto_impl::auto_impl(&, Arc)]
pub trait Block:
    Send + Sync + Unpin + Clone + Default + fmt::Debug + PartialEq + Eq + InMemorySize + MaybeSerde
{
    /// Header part of the block.
    type Header: BlockHeader + 'static;

    /// The block's body contains the transactions in the block.
    type Body: Send + Sync + Unpin + 'static;

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

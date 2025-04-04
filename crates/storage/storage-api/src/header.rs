use alloc::{sync::Arc, vec::Vec};
use alloy_eips::BlockHashOrNumber;
use alloy_primitives::{BlockHash, BlockNumber, U256};
use core::ops::RangeBounds;
use reth_primitives_traits::{NodePrimitives, SealedHeader};
use reth_storage_errors::provider::ProviderResult;

/// A helper type alias to access [`HeaderProvider::Header`].
pub type ProviderHeader<P> = <P as NodePrimitives>::BlockHeader;

/// Client trait for fetching `Header` related data.
pub trait HeaderProvider: NodePrimitives + Send + Sync {
    /// Check if block is known
    fn is_known(&self, block_hash: &BlockHash) -> ProviderResult<bool> {
        self.header(block_hash).map(|header| header.is_some())
    }

    /// Get header by block hash
    fn header(&self, block_hash: &BlockHash) -> ProviderResult<Option<Self::BlockHeader>>;

    /// Retrieves the header sealed by the given block hash.
    fn sealed_header_by_hash(
        &self,
        block_hash: BlockHash,
    ) -> ProviderResult<Option<SealedHeader<Self::BlockHeader>>> {
        Ok(self.header(&block_hash)?.map(|header| SealedHeader::new(header, block_hash)))
    }

    /// Get header by block number
    fn header_by_number(&self, num: u64) -> ProviderResult<Option<Self::BlockHeader>>;

    /// Get header by block number or hash
    fn header_by_hash_or_number(
        &self,
        hash_or_num: BlockHashOrNumber,
    ) -> ProviderResult<Option<Self::BlockHeader>> {
        match hash_or_num {
            BlockHashOrNumber::Hash(hash) => self.header(&hash),
            BlockHashOrNumber::Number(num) => self.header_by_number(num),
        }
    }

    /// Get total difficulty by block hash.
    fn header_td(&self, hash: &BlockHash) -> ProviderResult<Option<U256>>;

    /// Get total difficulty by block number.
    fn header_td_by_number(&self, number: BlockNumber) -> ProviderResult<Option<U256>>;

    /// Get headers in range of block numbers
    fn headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Self::BlockHeader>>;

    /// Get a single sealed header by block number.
    fn sealed_header(
        &self,
        number: BlockNumber,
    ) -> ProviderResult<Option<SealedHeader<Self::BlockHeader>>>;

    /// Get headers in range of block numbers.
    fn sealed_headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<SealedHeader<Self::BlockHeader>>> {
        self.sealed_headers_while(range, |_| true)
    }

    /// Get sealed headers while `predicate` returns `true` or the range is exhausted.
    fn sealed_headers_while(
        &self,
        range: impl RangeBounds<BlockNumber>,
        predicate: impl FnMut(&SealedHeader<Self::BlockHeader>) -> bool,
    ) -> ProviderResult<Vec<SealedHeader<Self::BlockHeader>>>;
}

impl<T: HeaderProvider> HeaderProvider for &T {
    fn header(&self, block_hash: &BlockHash) -> ProviderResult<Option<Self::BlockHeader>> {
        T::header(self, block_hash)
    }

    fn header_by_number(&self, num: u64) -> ProviderResult<Option<Self::BlockHeader>> {
        T::header_by_number(self, num)
    }

    fn header_td(&self, hash: &BlockHash) -> ProviderResult<Option<U256>> {
        T::header_td(self, hash)
    }

    fn header_td_by_number(&self, number: BlockNumber) -> ProviderResult<Option<U256>> {
        T::header_td_by_number(self, number)
    }

    fn headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Self::BlockHeader>> {
        T::headers_range(self, range)
    }

    fn sealed_header(
        &self,
        number: BlockNumber,
    ) -> ProviderResult<Option<SealedHeader<Self::BlockHeader>>> {
        T::sealed_header(self, number)
    }

    fn sealed_headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<SealedHeader<Self::BlockHeader>>> {
        T::sealed_headers_range(self, range)
    }

    fn sealed_headers_while(
        &self,
        range: impl RangeBounds<BlockNumber>,
        predicate: impl FnMut(&SealedHeader<Self::BlockHeader>) -> bool,
    ) -> ProviderResult<Vec<SealedHeader<Self::BlockHeader>>> {
        T::sealed_headers_while(self, range, predicate)
    }

    fn is_known(&self, block_hash: &BlockHash) -> ProviderResult<bool> {
        T::is_known(self, block_hash)
    }

    fn header_by_hash_or_number(
        &self,
        hash_or_num: BlockHashOrNumber,
    ) -> ProviderResult<Option<Self::BlockHeader>> {
        T::header_by_hash_or_number(self, hash_or_num)
    }
}

impl<T: HeaderProvider> HeaderProvider for Arc<T> {
    fn header(&self, block_hash: &BlockHash) -> ProviderResult<Option<Self::BlockHeader>> {
        T::header(self, block_hash)
    }

    fn header_by_number(&self, num: u64) -> ProviderResult<Option<Self::BlockHeader>> {
        T::header_by_number(self, num)
    }

    fn header_td(&self, hash: &BlockHash) -> ProviderResult<Option<U256>> {
        T::header_td(self, hash)
    }

    fn header_td_by_number(&self, number: BlockNumber) -> ProviderResult<Option<U256>> {
        T::header_td_by_number(self, number)
    }

    fn headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Self::BlockHeader>> {
        T::headers_range(self, range)
    }

    fn sealed_header(
        &self,
        number: BlockNumber,
    ) -> ProviderResult<Option<SealedHeader<Self::BlockHeader>>> {
        T::sealed_header(self, number)
    }

    fn sealed_headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<SealedHeader<Self::BlockHeader>>> {
        T::sealed_headers_range(self, range)
    }

    fn sealed_headers_while(
        &self,
        range: impl RangeBounds<BlockNumber>,
        predicate: impl FnMut(&SealedHeader<Self::BlockHeader>) -> bool,
    ) -> ProviderResult<Vec<SealedHeader<Self::BlockHeader>>> {
        T::sealed_headers_while(self, range, predicate)
    }

    fn is_known(&self, block_hash: &BlockHash) -> ProviderResult<bool> {
        T::is_known(self, block_hash)
    }

    fn header_by_hash_or_number(
        &self,
        hash_or_num: BlockHashOrNumber,
    ) -> ProviderResult<Option<Self::BlockHeader>> {
        T::header_by_hash_or_number(self, hash_or_num)
    }
}

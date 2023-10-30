use super::LoadedJarRef;
use crate::{BlockHashReader, BlockNumReader, HeaderProvider, TransactionsProvider};
use reth_db::{
    codecs::CompactU256,
    snapshot::{HeaderMask, SnapshotCursor},
    table::Decompress,
};
use reth_interfaces::{provider::ProviderError, RethResult};
use reth_primitives::{
    Address, BlockHash, BlockHashOrNumber, BlockNumber, ChainInfo, Header, SealedHeader,
    TransactionMeta, TransactionSigned, TransactionSignedNoHash, TxHash, TxNumber, B256, U256,
};
use std::ops::{Deref, Range, RangeBounds};

/// Provider over a specific `NippyJar` and range.
#[derive(Debug)]
pub struct SnapshotJarProvider<'a>(LoadedJarRef<'a>);

impl<'a> Deref for SnapshotJarProvider<'a> {
    type Target = LoadedJarRef<'a>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a> From<LoadedJarRef<'a>> for SnapshotJarProvider<'a> {
    fn from(value: LoadedJarRef<'a>) -> Self {
        SnapshotJarProvider(value)
    }
}

impl<'a> SnapshotJarProvider<'a> {
    /// Provides a cursor for more granular data access.
    pub fn cursor<'b>(&'b self) -> RethResult<SnapshotCursor<'a>>
    where
        'b: 'a,
    {
        SnapshotCursor::new(self.value(), self.mmap_handle())
    }
}

impl<'a> HeaderProvider for SnapshotJarProvider<'a> {
    fn header(&self, block_hash: &BlockHash) -> RethResult<Option<Header>> {
        Ok(self
            .cursor()?
            .get_two::<HeaderMask<Header, BlockHash>>(block_hash.into())?
            .filter(|(_, hash)| hash == block_hash)
            .map(|(header, _)| header))
    }

    fn header_by_number(&self, num: BlockNumber) -> RethResult<Option<Header>> {
        self.cursor()?.get_one::<HeaderMask<Header>>(num.into())
    }

    fn header_td(&self, block_hash: &BlockHash) -> RethResult<Option<U256>> {
        Ok(self
            .cursor()?
            .get_two::<HeaderMask<CompactU256, BlockHash>>(block_hash.into())?
            .filter(|(_, hash)| hash == block_hash)
            .map(|(td, _)| td.into()))
    }

    fn header_td_by_number(&self, num: BlockNumber) -> RethResult<Option<U256>> {
        Ok(self.cursor()?.get_one::<HeaderMask<CompactU256>>(num.into())?.map(Into::into))
    }

    fn headers_range(&self, range: impl RangeBounds<BlockNumber>) -> RethResult<Vec<Header>> {
        let range = to_range(range);

        let mut cursor = self.cursor()?;
        let mut headers = Vec::with_capacity((range.end - range.start) as usize);

        for num in range.start..range.end {
            match cursor.get_one::<HeaderMask<Header>>(num.into())? {
                Some(header) => headers.push(header),
                None => return Ok(headers),
            }
        }

        Ok(headers)
    }

    fn sealed_headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> RethResult<Vec<SealedHeader>> {
        let range = to_range(range);

        let mut cursor = self.cursor()?;
        let mut headers = Vec::with_capacity((range.end - range.start) as usize);

        for number in range.start..range.end {
            match cursor.get_two::<HeaderMask<Header, BlockHash>>(number.into())? {
                Some((header, hash)) => headers.push(header.seal(hash)),
                None => return Ok(headers),
            }
        }
        Ok(headers)
    }

    fn sealed_header(&self, number: BlockNumber) -> RethResult<Option<SealedHeader>> {
        Ok(self
            .cursor()?
            .get_two::<HeaderMask<Header, BlockHash>>(number.into())?
            .map(|(header, hash)| header.seal(hash)))
    }
}

impl<'a> BlockHashReader for SnapshotJarProvider<'a> {
    fn block_hash(&self, number: u64) -> RethResult<Option<B256>> {
        self.cursor()?.get_one::<HeaderMask<BlockHash>>(number.into())
    }

    fn canonical_hashes_range(
        &self,
        _start: BlockNumber,
        _end: BlockNumber,
    ) -> RethResult<Vec<B256>> {
        todo!()
    }
}

impl<'a> BlockNumReader for SnapshotJarProvider<'a> {
    fn chain_info(&self) -> RethResult<ChainInfo> {
        todo!()
    }

    fn best_block_number(&self) -> RethResult<BlockNumber> {
        todo!()
    }

    fn last_block_number(&self) -> RethResult<BlockNumber> {
        todo!()
    }

    fn block_number(&self, _hash: B256) -> RethResult<Option<BlockNumber>> {
        todo!()
    }
}

impl<'a> TransactionsProvider for SnapshotJarProvider<'a> {
    fn transaction_id(&self, _tx_hash: TxHash) -> RethResult<Option<TxNumber>> {
        todo!()
    }

    fn transaction_by_id(&self, num: TxNumber) -> RethResult<Option<TransactionSigned>> {
        TransactionSignedNoHash::decompress(
            self.cursor()?
                .row_by_number_with_cols((num - self.user_header().tx_start()) as usize, 0b1)?
                .ok_or_else(|| ProviderError::TransactionNotFound(num.into()))?[0],
        )
        .map(Into::into)
        .map(Some)
        .map_err(Into::into)
    }

    fn transaction_by_id_no_hash(
        &self,
        _id: TxNumber,
    ) -> RethResult<Option<TransactionSignedNoHash>> {
        todo!()
    }

    fn transaction_by_hash(&self, hash: TxHash) -> RethResult<Option<TransactionSigned>> {
        // WIP
        let mut cursor = self.cursor()?;

        let tx = TransactionSignedNoHash::decompress(
            cursor.row_by_key_with_cols(&hash.0, 0b1).unwrap().unwrap()[0],
        )
        .unwrap()
        .with_hash();

        if tx.hash() == hash {
            return Ok(Some(tx))
        } else {
            // check next snapshot
        }
        Ok(None)
    }

    fn transaction_by_hash_with_meta(
        &self,
        _hash: TxHash,
    ) -> RethResult<Option<(TransactionSigned, TransactionMeta)>> {
        todo!()
    }

    fn transaction_block(&self, _id: TxNumber) -> RethResult<Option<BlockNumber>> {
        todo!()
    }

    fn transactions_by_block(
        &self,
        _block_id: BlockHashOrNumber,
    ) -> RethResult<Option<Vec<TransactionSigned>>> {
        todo!()
    }

    fn transactions_by_block_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> RethResult<Vec<Vec<TransactionSigned>>> {
        todo!()
    }

    fn senders_by_tx_range(&self, _range: impl RangeBounds<TxNumber>) -> RethResult<Vec<Address>> {
        todo!()
    }

    fn transactions_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> RethResult<Vec<reth_primitives::TransactionSignedNoHash>> {
        todo!()
    }

    fn transaction_sender(&self, _id: TxNumber) -> RethResult<Option<Address>> {
        todo!()
    }
}

fn to_range<R: RangeBounds<u64>>(bounds: R) -> Range<u64> {
    let start = match bounds.start_bound() {
        std::ops::Bound::Included(&v) => v,
        std::ops::Bound::Excluded(&v) => v + 1,
        std::ops::Bound::Unbounded => 0,
    };

    let end = match bounds.end_bound() {
        std::ops::Bound::Included(&v) => v + 1,
        std::ops::Bound::Excluded(&v) => v,
        std::ops::Bound::Unbounded => u64::MAX,
    };

    start..end
}

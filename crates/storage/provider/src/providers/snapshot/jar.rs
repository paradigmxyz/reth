use super::LoadedJarRef;
use crate::{BlockHashReader, BlockNumReader, HeaderProvider, TransactionsProvider};
use reth_db::{
    table::{Decompress, Table},
    HeaderTD,
};
use reth_interfaces::{provider::ProviderError, RethResult};
use reth_nippy_jar::NippyJarCursor;
use reth_primitives::{
    snapshot::SegmentHeader, Address, BlockHash, BlockHashOrNumber, BlockNumber, ChainInfo, Header,
    SealedHeader, TransactionMeta, TransactionSigned, TransactionSignedNoHash, TxHash, TxNumber,
    B256, U256,
};
use std::ops::{Deref, RangeBounds};

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
    pub fn cursor<'b>(&'b self) -> RethResult<NippyJarCursor<'a, SegmentHeader>>
    where
        'b: 'a,
    {
        Ok(NippyJarCursor::with_handle(self.value(), self.mmap_handle())?)
    }
}

impl<'a> HeaderProvider for SnapshotJarProvider<'a> {
    fn header(&self, block_hash: &BlockHash) -> RethResult<Option<Header>> {
        // WIP
        let mut cursor = self.cursor()?;

        let header = Header::decompress(
            cursor.row_by_key_with_cols::<0b01, 2>(&block_hash.0).unwrap().unwrap()[0],
        )
        .unwrap();

        if &header.hash_slow() == block_hash {
            return Ok(Some(header))
        } else {
            // check next snapshot
        }
        Ok(None)
    }

    fn header_by_number(&self, num: BlockNumber) -> RethResult<Option<Header>> {
        Header::decompress(
            self.cursor()?
                .row_by_number_with_cols::<0b01, 2>(
                    (num - self.user_header().block_start()) as usize,
                )?
                .ok_or(ProviderError::HeaderNotFound(num.into()))?[0],
        )
        .map(Some)
        .map_err(Into::into)
    }

    fn header_td(&self, block_hash: &BlockHash) -> RethResult<Option<U256>> {
        // WIP
        let mut cursor = NippyJarCursor::with_handle(self.value(), self.mmap_handle())?;

        let row = cursor.row_by_key_with_cols::<0b11, 2>(&block_hash.0).unwrap().unwrap();

        let header = Header::decompress(row[0]).unwrap();
        let td = <HeaderTD as Table>::Value::decompress(row[1]).unwrap();

        if &header.hash_slow() == block_hash {
            return Ok(Some(td.0))
        } else {
            // check next snapshot
        }
        Ok(None)
    }

    fn header_td_by_number(&self, _number: BlockNumber) -> RethResult<Option<U256>> {
        unimplemented!();
    }

    fn headers_range(&self, _range: impl RangeBounds<BlockNumber>) -> RethResult<Vec<Header>> {
        unimplemented!();
    }

    fn sealed_headers_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> RethResult<Vec<SealedHeader>> {
        unimplemented!();
    }

    fn sealed_header(&self, _number: BlockNumber) -> RethResult<Option<SealedHeader>> {
        unimplemented!();
    }
}

impl<'a> BlockHashReader for SnapshotJarProvider<'a> {
    fn block_hash(&self, _number: u64) -> RethResult<Option<B256>> {
        todo!()
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
                .row_by_number_with_cols::<0b1, 1>((num - self.user_header().tx_start()) as usize)?
                .ok_or(ProviderError::TransactionNotFound(num.into()))?[0],
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
            cursor.row_by_key_with_cols::<0b1, 1>(&hash.0).unwrap().unwrap()[0],
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

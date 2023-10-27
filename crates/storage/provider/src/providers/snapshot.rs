use crate::{BlockHashReader, BlockNumReader, HeaderProvider, TransactionsProvider};
use dashmap::DashMap;
use reth_db::{
    table::{Decompress, Table},
    HeaderTD,
};
use reth_interfaces::{provider::ProviderError, RethResult};
use reth_nippy_jar::{NippyJar, NippyJarCursor};
use reth_primitives::{
    snapshot::{SegmentHeader, BLOCKS_PER_SNAPSHOT},
    Address, BlockHash, BlockHashOrNumber, BlockNumber, ChainInfo, Header, SealedHeader,
    SnapshotSegment, TransactionMeta, TransactionSigned, TransactionSignedNoHash, TxHash, TxNumber,
    B256, U256,
};
use std::{ops::RangeBounds, path::PathBuf};

/// Alias type for each specific `NippyJar`.
type NippyJarRef<'a> =
    dashmap::mapref::one::Ref<'a, (u64, SnapshotSegment), NippyJar<SegmentHeader>>;

/// SnapshotProvider
#[derive(Debug, Default)]
pub struct SnapshotProvider {
    /// Maintains a map which allows for concurrent access to different `NippyJars`, over different
    /// segments and ranges.
    map: DashMap<(BlockNumber, SnapshotSegment), NippyJar<SegmentHeader>>,
}

impl SnapshotProvider {
    /// Gets the provider of the requested segment and range.
    pub fn get_segment_provider(
        &self,
        segment: SnapshotSegment,
        block: BlockNumber,
        mut path: Option<PathBuf>,
    ) -> RethResult<SnapshotJarProvider<'_>> {
        // TODO this invalidates custom length snapshots.
        let snapshot = block / BLOCKS_PER_SNAPSHOT;
        let key = (snapshot, segment);

        if let Some(jar) = self.map.get(&key) {
            return Ok(SnapshotJarProvider { jar })
        }

        if let Some(path) = &path {
            let jar = NippyJar::load(path)?;
            self.map.insert(key, jar);
        } else {
            path = Some(segment.filename(
                &((snapshot * BLOCKS_PER_SNAPSHOT)..=((snapshot + 1) * BLOCKS_PER_SNAPSHOT - 1)),
            ));
        }

        self.get_segment_provider(segment, block, path)
    }
}

impl HeaderProvider for SnapshotProvider {
    fn header(&self, _block_hash: &BlockHash) -> RethResult<Option<Header>> {
        todo!()
    }

    fn header_by_number(&self, num: BlockNumber) -> RethResult<Option<Header>> {
        self.get_segment_provider(SnapshotSegment::Headers, num, None)?.header_by_number(num)
    }

    fn header_td(&self, _block_hash: &BlockHash) -> RethResult<Option<U256>> {
        todo!()
    }

    fn header_td_by_number(&self, _number: BlockNumber) -> RethResult<Option<U256>> {
        todo!();
    }

    fn headers_range(&self, _range: impl RangeBounds<BlockNumber>) -> RethResult<Vec<Header>> {
        todo!();
    }

    fn sealed_headers_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> RethResult<Vec<SealedHeader>> {
        todo!();
    }

    fn sealed_header(&self, _number: BlockNumber) -> RethResult<Option<SealedHeader>> {
        todo!();
    }
}

impl BlockHashReader for SnapshotProvider {
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

impl BlockNumReader for SnapshotProvider {
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

impl TransactionsProvider for SnapshotProvider {
    fn transaction_id(&self, _tx_hash: TxHash) -> RethResult<Option<TxNumber>> {
        todo!()
    }

    fn transaction_by_id(&self, num: TxNumber) -> RethResult<Option<TransactionSigned>> {
        // TODO `num` is provided after checking the index
        let block_num = num;
        self.get_segment_provider(SnapshotSegment::Transactions, block_num, None)?
            .transaction_by_id(num)
    }

    fn transaction_by_id_no_hash(
        &self,
        _id: TxNumber,
    ) -> RethResult<Option<TransactionSignedNoHash>> {
        todo!()
    }

    fn transaction_by_hash(&self, _hash: TxHash) -> RethResult<Option<TransactionSigned>> {
        todo!()
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

/// Provider over a specific `NippyJar` and range.
#[derive(Debug)]
pub struct SnapshotJarProvider<'a> {
    /// Reference to a value on [`SnapshotProvider`]
    pub jar: NippyJarRef<'a>,
}

impl<'a> SnapshotJarProvider<'a> {
    /// Provides a cursor for more granular data access.
    pub fn cursor<'b>(&'b self) -> RethResult<NippyJarCursor<'a, SegmentHeader>>
    where
        'b: 'a,
    {
        Ok(NippyJarCursor::new(self.jar.value())?)
    }
}

impl<'a> HeaderProvider for SnapshotJarProvider<'a> {
    fn header(&self, block_hash: &BlockHash) -> RethResult<Option<Header>> {
        // WIP
        let mut cursor = NippyJarCursor::new(self.jar.value())?;

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
            NippyJarCursor::new(self.jar.value())?
                .row_by_number_with_cols::<0b01, 2>(
                    (num - self.jar.user_header().block_start()) as usize,
                )?
                .ok_or(ProviderError::HeaderNotFound(num.into()))?[0],
        )
        .map(Some)
        .map_err(Into::into)
    }

    fn header_td(&self, block_hash: &BlockHash) -> RethResult<Option<U256>> {
        // WIP
        let mut cursor = NippyJarCursor::new(self.jar.value())?;

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
            NippyJarCursor::new(self.jar.value())?
                .row_by_number_with_cols::<0b1, 1>(
                    (num - self.jar.user_header().tx_start()) as usize,
                )?
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
        let mut cursor = NippyJarCursor::new(self.jar.value())?;

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

#[cfg(test)]
mod test {
    use super::*;
    use crate::ProviderFactory;
    use rand::{self, seq::SliceRandom};
    use reth_db::{
        cursor::DbCursorRO,
        database::Database,
        snapshot::create_snapshot_T1_T2,
        test_utils::create_test_rw_db,
        transaction::{DbTx, DbTxMut},
        CanonicalHeaders, DatabaseError, HeaderNumbers, HeaderTD, Headers, RawTable,
    };
    use reth_interfaces::test_utils::generators::{self, random_header_range};
    use reth_nippy_jar::NippyJar;
    use reth_primitives::{B256, MAINNET};

    #[test]
    fn test_snap() {
        // Ranges
        let row_count = 100u64;
        let range = 0..=(row_count - 1);
        let segment_header = SegmentHeader::new(range.clone(), range.clone());

        // Data sources
        let db = create_test_rw_db();
        let factory = ProviderFactory::new(&db, MAINNET.clone());
        let snap_file = tempfile::NamedTempFile::new().unwrap();

        // Setup data
        let mut headers = random_header_range(
            &mut generators::rng(),
            *range.start()..(*range.end() + 1),
            B256::random(),
        );

        db.update(|tx| -> Result<(), DatabaseError> {
            let mut td = U256::ZERO;
            for header in headers.clone() {
                td += header.header.difficulty;
                let hash = header.hash();

                tx.put::<CanonicalHeaders>(header.number, hash)?;
                tx.put::<Headers>(header.number, header.clone().unseal())?;
                tx.put::<HeaderTD>(header.number, td.into())?;
                tx.put::<HeaderNumbers>(hash, header.number)?;
            }
            Ok(())
        })
        .unwrap()
        .unwrap();

        // Create Snapshot
        {
            let with_compression = true;
            let with_filter = true;

            let mut nippy_jar = NippyJar::new(2, snap_file.path(), segment_header);

            if with_compression {
                nippy_jar = nippy_jar.with_zstd(false, 0);
            }

            if with_filter {
                nippy_jar = nippy_jar.with_cuckoo_filter(row_count as usize + 10).with_fmph();
            }

            let tx = db.tx().unwrap();

            // Hacky type inference. TODO fix
            let mut none_vec = Some(vec![vec![vec![0u8]].into_iter()]);
            let _ = none_vec.take();

            // Generate list of hashes for filters & PHF
            let mut cursor = tx.cursor_read::<RawTable<CanonicalHeaders>>().unwrap();
            let hashes = cursor
                .walk(None)
                .unwrap()
                .map(|row| row.map(|(_key, value)| value.into_value()).map_err(|e| e.into()));

            create_snapshot_T1_T2::<Headers, HeaderTD, BlockNumber, SegmentHeader>(
                &tx,
                range,
                None,
                none_vec,
                Some(hashes),
                row_count as usize,
                &mut nippy_jar,
            )
            .unwrap();
        }

        // Use providers to query Header data and compare if it matches
        {
            let db_provider = factory.provider().unwrap();
            let manager = SnapshotProvider::default();
            let jar_provider = manager
                .get_segment_provider(SnapshotSegment::Headers, 0, Some(snap_file.path().into()))
                .unwrap();

            assert!(!headers.is_empty());

            // Shuffled for chaos.
            headers.shuffle(&mut generators::rng());

            for header in headers {
                let header_hash = header.hash();
                let header = header.unseal();

                // Compare Header
                assert_eq!(header, db_provider.header(&header_hash).unwrap().unwrap());
                assert_eq!(header, jar_provider.header(&header_hash).unwrap().unwrap());

                // Compare HeaderTD
                assert_eq!(
                    db_provider.header_td(&header_hash).unwrap().unwrap(),
                    jar_provider.header_td(&header_hash).unwrap().unwrap()
                );
            }
        }
    }
}

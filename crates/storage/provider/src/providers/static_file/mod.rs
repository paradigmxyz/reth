mod manager;
pub use manager::{StaticFileAccess, StaticFileProvider, StaticFileWriter};

mod jar;
pub use jar::StaticFileJarProvider;

mod writer;
pub use writer::{StaticFileProviderRW, StaticFileProviderRWRefMut};

mod metrics;

use reth_nippy_jar::NippyJar;
use reth_primitives::{static_file::SegmentHeader, StaticFileSegment};
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use std::{ops::Deref, sync::Arc};

/// Alias type for each specific `NippyJar`.
type LoadedJarRef<'a> = dashmap::mapref::one::Ref<'a, (u64, StaticFileSegment), LoadedJar>;

/// Helper type to reuse an associated static file mmap handle on created cursors.
#[derive(Debug)]
pub struct LoadedJar {
    jar: NippyJar<SegmentHeader>,
    mmap_handle: Arc<reth_nippy_jar::DataReader>,
}

impl LoadedJar {
    fn new(jar: NippyJar<SegmentHeader>) -> ProviderResult<Self> {
        match jar.open_data_reader() {
            Ok(data_reader) => {
                let mmap_handle = Arc::new(data_reader);
                Ok(Self { jar, mmap_handle })
            }
            Err(e) => Err(ProviderError::NippyJar(e.to_string())),
        }
    }

    /// Returns a clone of the mmap handle that can be used to instantiate a cursor.
    fn mmap_handle(&self) -> Arc<reth_nippy_jar::DataReader> {
        self.mmap_handle.clone()
    }

    const fn segment(&self) -> StaticFileSegment {
        self.jar.user_header().segment()
    }
}

impl Deref for LoadedJar {
    type Target = NippyJar<SegmentHeader>;
    fn deref(&self) -> &Self::Target {
        &self.jar
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_utils::create_test_provider_factory, HeaderProvider};
    use alloy_primitives::{BlockHash, TxNumber, B256, U256};
    use rand::seq::SliceRandom;
    use reth_db::{
        test_utils::create_test_static_files_dir, CanonicalHeaders, HeaderNumbers,
        HeaderTerminalDifficulties, Headers,
    };
    use reth_db_api::transaction::DbTxMut;
    use reth_primitives::{
        static_file::{find_fixed_range, SegmentRangeInclusive, DEFAULT_BLOCKS_PER_STATIC_FILE},
        Header, Receipt, TransactionSignedNoHash,
    };
    use reth_storage_api::{ReceiptProvider, TransactionsProvider};
    use reth_testing_utils::generators::{self, random_header_range};
    use std::{fmt::Debug, fs, ops::Range, path::Path};

    fn assert_eyre<T: PartialEq + Debug>(got: T, expected: T, msg: &str) -> eyre::Result<()> {
        if got != expected {
            eyre::bail!("{msg} | got: {got:?} expected: {expected:?})");
        }
        Ok(())
    }

    #[test]
    fn test_snap() {
        // Ranges
        let row_count = 100u64;
        let range = 0..=(row_count - 1);

        // Data sources
        let factory = create_test_provider_factory();
        let static_files_path = tempfile::tempdir().unwrap();
        let static_file = static_files_path.path().join(
            StaticFileSegment::Headers
                .filename(&find_fixed_range(*range.end(), DEFAULT_BLOCKS_PER_STATIC_FILE)),
        );

        // Setup data
        let mut headers = random_header_range(
            &mut generators::rng(),
            *range.start()..(*range.end() + 1),
            B256::random(),
        );

        let mut provider_rw = factory.provider_rw().unwrap();
        let tx = provider_rw.tx_mut();
        let mut td = U256::ZERO;
        for header in headers.clone() {
            td += header.header().difficulty;
            let hash = header.hash();

            tx.put::<CanonicalHeaders>(header.number, hash).unwrap();
            tx.put::<Headers>(header.number, header.clone().unseal()).unwrap();
            tx.put::<HeaderTerminalDifficulties>(header.number, td.into()).unwrap();
            tx.put::<HeaderNumbers>(hash, header.number).unwrap();
        }
        provider_rw.commit().unwrap();

        // Create StaticFile
        {
            let manager = StaticFileProvider::read_write(static_files_path.path()).unwrap();
            let mut writer = manager.latest_writer(StaticFileSegment::Headers).unwrap();
            let mut td = U256::ZERO;

            for header in headers.clone() {
                td += header.header().difficulty;
                let hash = header.hash();
                writer.append_header(&header.unseal(), td, &hash).unwrap();
            }
            writer.commit().unwrap();
        }

        // Use providers to query Header data and compare if it matches
        {
            let db_provider = factory.provider().unwrap();
            let manager = StaticFileProvider::read_write(static_files_path.path()).unwrap();
            let jar_provider = manager
                .get_segment_provider_from_block(StaticFileSegment::Headers, 0, Some(&static_file))
                .unwrap();

            assert!(!headers.is_empty());

            // Shuffled for chaos.
            headers.shuffle(&mut generators::rng());

            for header in headers {
                let header_hash = header.hash();
                let header = header.unseal();

                // Compare Header
                assert_eq!(header, db_provider.header(&header_hash).unwrap().unwrap());
                assert_eq!(header, jar_provider.header_by_number(header.number).unwrap().unwrap());

                // Compare HeaderTerminalDifficulties
                assert_eq!(
                    db_provider.header_td(&header_hash).unwrap().unwrap(),
                    jar_provider.header_td_by_number(header.number).unwrap().unwrap()
                );
            }
        }
    }

    #[test]
    fn test_header_truncation() {
        let (static_dir, _) = create_test_static_files_dir();

        let blocks_per_file = 10; // Number of headers per file
        let files_per_range = 3; // Number of files per range (data/conf/offset files)
        let file_set_count = 3; // Number of sets of files to create
        let initial_file_count = files_per_range * file_set_count + 1; // Includes lockfile
        let tip = blocks_per_file * file_set_count - 1; // Initial highest block (29 in this case)

        // [ Headers Creation and Commit ]
        {
            let sf_rw = StaticFileProvider::read_write(&static_dir)
                .expect("Failed to create static file provider")
                .with_custom_blocks_per_file(blocks_per_file);

            let mut header_writer = sf_rw.latest_writer(StaticFileSegment::Headers).unwrap();

            // Append headers from 0 to the tip (29) and commit
            let mut header = Header::default();
            for num in 0..=tip {
                header.number = num;
                header_writer
                    .append_header(&header, U256::default(), &BlockHash::default())
                    .unwrap();
            }
            header_writer.commit().unwrap();
        }

        // Helper function to prune headers and validate truncation results
        fn prune_and_validate(
            writer: &mut StaticFileProviderRWRefMut<'_>,
            sf_rw: &StaticFileProvider,
            static_dir: impl AsRef<Path>,
            prune_count: u64,
            expected_tip: Option<u64>,
            expected_file_count: u64,
        ) -> eyre::Result<()> {
            writer.prune_headers(prune_count)?;
            writer.commit()?;

            // Validate the highest block after pruning
            assert_eyre(
                sf_rw.get_highest_static_file_block(StaticFileSegment::Headers),
                expected_tip,
                "block mismatch",
            )?;

            if let Some(id) = expected_tip {
                assert_eyre(
                    sf_rw.header_by_number(id)?.map(|h| h.number),
                    expected_tip,
                    "header mismatch",
                )?;
            }

            // Validate the number of files remaining in the directory
            assert_eyre(
                fs::read_dir(static_dir)?.count(),
                expected_file_count as usize,
                "file count mismatch",
            )?;

            Ok(())
        }

        // [ Test Cases ]
        type PruneCount = u64;
        type ExpectedTip = u64;
        type ExpectedFileCount = u64;
        let mut tmp_tip = tip;
        let test_cases: Vec<(PruneCount, Option<ExpectedTip>, ExpectedFileCount)> = vec![
            // Case 0: Pruning 1 header
            {
                tmp_tip -= 1;
                (1, Some(tmp_tip), initial_file_count)
            },
            // Case 1: Pruning remaining rows from file should result in its deletion
            {
                tmp_tip -= blocks_per_file - 1;
                (blocks_per_file - 1, Some(tmp_tip), initial_file_count - files_per_range)
            },
            // Case 2: Pruning more headers than a single file has (tip reduced by
            // blocks_per_file + 1) should result in a file set deletion
            {
                tmp_tip -= blocks_per_file + 1;
                (blocks_per_file + 1, Some(tmp_tip), initial_file_count - files_per_range * 2)
            },
            // Case 3: Pruning all remaining headers from the file except the genesis header
            {
                (
                    tmp_tip,
                    Some(0),             // Only genesis block remains
                    files_per_range + 1, // The file set with block 0 should remain
                )
            },
            // Case 4: Pruning the genesis header (should not delete the file set with block 0)
            {
                (
                    1,
                    None,                // No blocks left
                    files_per_range + 1, // The file set with block 0 remains
                )
            },
        ];

        // Test cases execution
        {
            let sf_rw = StaticFileProvider::read_write(&static_dir)
                .expect("Failed to create static file provider")
                .with_custom_blocks_per_file(blocks_per_file);

            assert_eq!(sf_rw.get_highest_static_file_block(StaticFileSegment::Headers), Some(tip));
            assert_eq!(
                fs::read_dir(static_dir.as_ref()).unwrap().count(),
                initial_file_count as usize
            );

            let mut header_writer = sf_rw.latest_writer(StaticFileSegment::Headers).unwrap();

            for (case, (prune_count, expected_tip, expected_file_count)) in
                test_cases.into_iter().enumerate()
            {
                prune_and_validate(
                    &mut header_writer,
                    &sf_rw,
                    &static_dir,
                    prune_count,
                    expected_tip,
                    expected_file_count,
                )
                .map_err(|err| eyre::eyre!("Test case {case}: {err}"))
                .unwrap();
            }
        }
    }

    /// 3 block ranges are built
    ///
    /// for `blocks_per_file = 10`:
    /// * `0..=9` : except genesis, every block has a tx/receipt
    /// * `10..=19`: no txs/receipts
    /// * `20..=29`: only one tx/receipt
    fn setup_tx_based_scenario(
        sf_rw: &StaticFileProvider,
        segment: StaticFileSegment,
        blocks_per_file: u64,
    ) {
        fn setup_block_ranges(
            writer: &mut StaticFileProviderRWRefMut<'_>,
            sf_rw: &StaticFileProvider,
            segment: StaticFileSegment,
            block_range: &Range<u64>,
            mut tx_count: u64,
            next_tx_num: &mut u64,
        ) {
            let mut receipt = Receipt::default();
            let mut tx = TransactionSignedNoHash::default();

            for block in block_range.clone() {
                writer.increment_block(block).unwrap();

                // Append transaction/receipt if there's still a transaction count to append
                if tx_count > 0 {
                    if segment.is_receipts() {
                        // Used as ID for validation
                        receipt.cumulative_gas_used = *next_tx_num;
                        writer.append_receipt(*next_tx_num, &receipt).unwrap();
                    } else {
                        // Used as ID for validation
                        tx.transaction.set_nonce(*next_tx_num);
                        writer.append_transaction(*next_tx_num, &tx).unwrap();
                    }
                    *next_tx_num += 1;
                    tx_count -= 1;
                }
            }
            writer.commit().unwrap();

            // Calculate expected values based on the range and transactions
            let expected_block = block_range.end - 1;
            let expected_tx = if tx_count == 0 { *next_tx_num - 1 } else { *next_tx_num };

            // Perform assertions after processing the blocks
            assert_eq!(sf_rw.get_highest_static_file_block(segment), Some(expected_block),);
            assert_eq!(sf_rw.get_highest_static_file_tx(segment), Some(expected_tx),);
        }

        // Define the block ranges and transaction counts as vectors
        let block_ranges = [
            0..blocks_per_file,
            blocks_per_file..blocks_per_file * 2,
            blocks_per_file * 2..blocks_per_file * 3,
        ];

        let tx_counts = [
            blocks_per_file - 1, // First range: tx per block except genesis
            0,                   // Second range: no transactions
            1,                   // Third range: 1 transaction in the second block
        ];

        let mut writer = sf_rw.latest_writer(segment).unwrap();
        let mut next_tx_num = 0;

        // Loop through setup scenarios
        for (block_range, tx_count) in block_ranges.iter().zip(tx_counts.iter()) {
            setup_block_ranges(
                &mut writer,
                sf_rw,
                segment,
                block_range,
                *tx_count,
                &mut next_tx_num,
            );
        }

        // Ensure that scenario was properly setup
        let expected_tx_ranges = vec![
            Some(SegmentRangeInclusive::new(0, 8)),
            None,
            Some(SegmentRangeInclusive::new(9, 9)),
        ];

        block_ranges.iter().zip(expected_tx_ranges).for_each(|(block_range, expected_tx_range)| {
            assert_eq!(
                sf_rw
                    .get_segment_provider_from_block(segment, block_range.start, None)
                    .unwrap()
                    .user_header()
                    .tx_range(),
                expected_tx_range.as_ref()
            );
        });

        // Ensure transaction index
        let tx_index = sf_rw.tx_index().read();
        let expected_tx_index =
            vec![(8, SegmentRangeInclusive::new(0, 9)), (9, SegmentRangeInclusive::new(20, 29))];
        assert_eq!(
            tx_index.get(&segment).map(|index| index.iter().map(|(k, v)| (*k, *v)).collect()),
            (!expected_tx_index.is_empty()).then_some(expected_tx_index),
            "tx index mismatch",
        );
    }

    #[test]
    fn test_tx_based_truncation() {
        let segments = [StaticFileSegment::Transactions, StaticFileSegment::Receipts];
        let blocks_per_file = 10; // Number of blocks per file
        let files_per_range = 3; // Number of files per range (data/conf/offset files)
        let file_set_count = 3; // Number of sets of files to create
        let initial_file_count = files_per_range * file_set_count + 1; // Includes lockfile

        #[allow(clippy::too_many_arguments)]
        fn prune_and_validate(
            sf_rw: &StaticFileProvider,
            static_dir: impl AsRef<Path>,
            segment: StaticFileSegment,
            prune_count: u64,
            last_block: u64,
            expected_tx_tip: Option<u64>,
            expected_file_count: i32,
            expected_tx_index: Vec<(TxNumber, SegmentRangeInclusive)>,
        ) -> eyre::Result<()> {
            let mut writer = sf_rw.latest_writer(segment)?;

            // Prune transactions or receipts based on the segment type
            if segment.is_receipts() {
                writer.prune_receipts(prune_count, last_block)?;
            } else {
                writer.prune_transactions(prune_count, last_block)?;
            }
            writer.commit()?;

            // Verify the highest block and transaction tips
            assert_eyre(
                sf_rw.get_highest_static_file_block(segment),
                Some(last_block),
                "block mismatch",
            )?;
            assert_eyre(sf_rw.get_highest_static_file_tx(segment), expected_tx_tip, "tx mismatch")?;

            // Verify that transactions and receipts are returned correctly. Uses
            // cumulative_gas_used & nonce as ids.
            if let Some(id) = expected_tx_tip {
                if segment.is_receipts() {
                    assert_eyre(
                        expected_tx_tip,
                        sf_rw.receipt(id)?.map(|r| r.cumulative_gas_used),
                        "tx mismatch",
                    )?;
                } else {
                    assert_eyre(
                        expected_tx_tip,
                        sf_rw.transaction_by_id(id)?.map(|t| t.nonce()),
                        "tx mismatch",
                    )?;
                }
            }

            // Ensure the file count has reduced as expected
            assert_eyre(
                fs::read_dir(static_dir)?.count(),
                expected_file_count as usize,
                "file count mismatch",
            )?;

            // Ensure that the inner tx index (max_tx -> block range) is as expected
            let tx_index = sf_rw.tx_index().read();
            assert_eyre(
                tx_index.get(&segment).map(|index| index.iter().map(|(k, v)| (*k, *v)).collect()),
                (!expected_tx_index.is_empty()).then_some(expected_tx_index),
                "tx index mismatch",
            )?;

            Ok(())
        }

        for segment in segments {
            let (static_dir, _) = create_test_static_files_dir();

            let sf_rw = StaticFileProvider::read_write(&static_dir)
                .expect("Failed to create static file provider")
                .with_custom_blocks_per_file(blocks_per_file);

            setup_tx_based_scenario(&sf_rw, segment, blocks_per_file);

            let sf_rw = StaticFileProvider::read_write(&static_dir)
                .expect("Failed to create static file provider")
                .with_custom_blocks_per_file(blocks_per_file);
            let highest_tx = sf_rw.get_highest_static_file_tx(segment).unwrap();

            // Test cases
            // [prune_count, last_block, expected_tx_tip, expected_file_count, expected_tx_index)
            let test_cases = vec![
                // Case 0: 20..=29 has only one tx. Prune the only tx of the block range.
                // It ensures that the file is not deleted even though there are no rows, since the
                // `last_block` which is passed to the prune method is the first
                // block of the range.
                (
                    1,
                    blocks_per_file * 2,
                    Some(highest_tx - 1),
                    initial_file_count,
                    vec![(highest_tx - 1, SegmentRangeInclusive::new(0, 9))],
                ),
                // Case 1: 10..=19 has no txs. There are no txes in the whole block range, but want
                // to unwind to block 9. Ensures that the 20..=29 and 10..=19 files
                // are deleted.
                (
                    0,
                    blocks_per_file - 1,
                    Some(highest_tx - 1),
                    files_per_range + 1, // includes lockfile
                    vec![(highest_tx - 1, SegmentRangeInclusive::new(0, 9))],
                ),
                // Case 2: Prune most txs up to block 1.
                (
                    highest_tx - 1,
                    1,
                    Some(0),
                    files_per_range + 1,
                    vec![(0, SegmentRangeInclusive::new(0, 1))],
                ),
                // Case 3: Prune remaining tx and ensure that file is not deleted.
                (1, 0, None, files_per_range + 1, vec![]),
            ];

            // Loop through test cases
            for (
                case,
                (prune_count, last_block, expected_tx_tip, expected_file_count, expected_tx_index),
            ) in test_cases.into_iter().enumerate()
            {
                prune_and_validate(
                    &sf_rw,
                    &static_dir,
                    segment,
                    prune_count,
                    last_block,
                    expected_tx_tip,
                    expected_file_count,
                    expected_tx_index,
                )
                .map_err(|err| eyre::eyre!("Test case {case}: {err}"))
                .unwrap();
            }
        }
    }
}

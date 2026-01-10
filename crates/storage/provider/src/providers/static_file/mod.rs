mod manager;
pub use manager::{
    StaticFileAccess, StaticFileProvider, StaticFileProviderBuilder, StaticFileWriter,
};

mod jar;
pub use jar::StaticFileJarProvider;

mod writer;
pub use writer::{StaticFileProviderRW, StaticFileProviderRWRefMut};

mod metrics;
use reth_nippy_jar::NippyJar;
use reth_static_file_types::{SegmentHeader, StaticFileSegment};
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
            Err(e) => Err(ProviderError::other(e)),
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
    use crate::{
        providers::static_file::manager::StaticFileProviderBuilder,
        test_utils::create_test_provider_factory, HeaderProvider, StaticFileProviderFactory,
    };
    use alloy_consensus::{Header, SignableTransaction, Transaction, TxLegacy};
    use alloy_primitives::{Address, BlockHash, Signature, TxNumber, B256, U160, U256};
    use rand::seq::SliceRandom;
    use reth_db::{
        models::{AccountBeforeTx, StorageBeforeTx},
        test_utils::create_test_static_files_dir,
    };
    use reth_db_api::{transaction::DbTxMut, CanonicalHeaders, HeaderNumbers, Headers};
    use reth_ethereum_primitives::{EthPrimitives, Receipt, TransactionSigned};
    use reth_primitives_traits::Account;
    use reth_static_file_types::{
        find_fixed_range, SegmentRangeInclusive, DEFAULT_BLOCKS_PER_STATIC_FILE,
    };
    use reth_storage_api::{
        ChangeSetReader, ReceiptProvider, StorageChangeSetReader, TransactionsProvider,
    };
    use reth_testing_utils::generators::{self, random_header_range};
    use std::{collections::BTreeMap, fmt::Debug, fs, ops::Range, path::Path};

    fn assert_eyre<T: PartialEq + Debug>(got: T, expected: T, msg: &str) -> eyre::Result<()> {
        if got != expected {
            eyre::bail!("{msg} | got: {got:?} expected: {expected:?}");
        }
        Ok(())
    }

    #[test]
    fn test_static_files() {
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
        for header in headers.clone() {
            let hash = header.hash();

            tx.put::<CanonicalHeaders>(header.number, hash).unwrap();
            tx.put::<Headers>(header.number, header.clone_header()).unwrap();
            tx.put::<HeaderNumbers>(hash, header.number).unwrap();
        }
        provider_rw.commit().unwrap();

        // Create StaticFile
        {
            let manager = factory.static_file_provider();
            let mut writer = manager.latest_writer(StaticFileSegment::Headers).unwrap();

            for header in headers.clone() {
                let hash = header.hash();
                writer.append_header(&header.unseal(), &hash).unwrap();
            }
            writer.commit().unwrap();
        }

        // Use providers to query Header data and compare if it matches
        {
            let db_provider = factory.provider().unwrap();
            let manager = db_provider.static_file_provider();
            let jar_provider = manager
                .get_segment_provider_for_block(StaticFileSegment::Headers, 0, Some(&static_file))
                .unwrap();

            assert!(!headers.is_empty());

            // Shuffled for chaos.
            headers.shuffle(&mut generators::rng());

            for header in headers {
                let header_hash = header.hash();
                let header = header.unseal();

                // Compare Header
                assert_eq!(header, db_provider.header(header_hash).unwrap().unwrap());
                assert_eq!(header, jar_provider.header_by_number(header.number).unwrap().unwrap());
            }
        }
    }

    #[test]
    fn test_header_truncation() {
        let (static_dir, _) = create_test_static_files_dir();

        let blocks_per_file = 10; // Number of headers per file
        let files_per_range = 3; // Number of files per range (data/conf/offset files)
        let file_set_count = 3; // Number of sets of files to create
        let initial_file_count = files_per_range * file_set_count;
        let tip = blocks_per_file * file_set_count - 1; // Initial highest block (29 in this case)

        // [ Headers Creation and Commit ]
        {
            let sf_rw: StaticFileProvider<EthPrimitives> =
                StaticFileProviderBuilder::read_write(&static_dir)
                    .with_blocks_per_file(blocks_per_file)
                    .build()
                    .expect("Failed to build static file provider");

            let mut header_writer = sf_rw.latest_writer(StaticFileSegment::Headers).unwrap();

            // Append headers from 0 to the tip (29) and commit
            let mut header = Header::default();
            for num in 0..=tip {
                header.number = num;
                header_writer.append_header(&header, &BlockHash::default()).unwrap();
            }
            header_writer.commit().unwrap();
        }

        // Helper function to prune headers and validate truncation results
        fn prune_and_validate(
            writer: &mut StaticFileProviderRWRefMut<'_, EthPrimitives>,
            sf_rw: &StaticFileProvider<EthPrimitives>,
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
                count_files_without_lockfile(static_dir)?,
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
                    Some(0),         // Only genesis block remains
                    files_per_range, // The file set with block 0 should remain
                )
            },
            // Case 4: Pruning the genesis header (should not delete the file set with block 0)
            {
                (
                    1,
                    None,            // No blocks left
                    files_per_range, // The file set with block 0 remains
                )
            },
        ];

        // Test cases execution
        {
            let sf_rw = StaticFileProviderBuilder::read_write(&static_dir)
                .with_blocks_per_file(blocks_per_file)
                .build()
                .expect("Failed to build static file provider");

            assert_eq!(sf_rw.get_highest_static_file_block(StaticFileSegment::Headers), Some(tip));
            assert_eq!(
                count_files_without_lockfile(static_dir.as_ref()).unwrap(),
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
        sf_rw: &StaticFileProvider<EthPrimitives>,
        segment: StaticFileSegment,
        blocks_per_file: u64,
    ) {
        fn setup_block_ranges(
            writer: &mut StaticFileProviderRWRefMut<'_, EthPrimitives>,
            sf_rw: &StaticFileProvider<EthPrimitives>,
            segment: StaticFileSegment,
            block_range: &Range<u64>,
            mut tx_count: u64,
            next_tx_num: &mut u64,
        ) {
            let mut receipt = Receipt::default();
            let mut tx = TxLegacy::default();

            for block in block_range.clone() {
                writer.increment_block(block).unwrap();

                // Append transaction/receipt if there's still a transaction count to append
                if tx_count > 0 {
                    match segment {
                        StaticFileSegment::Headers |
                        StaticFileSegment::AccountChangeSets |
                        StaticFileSegment::StorageChangeSets => {
                            panic!("non tx based segment")
                        }
                        StaticFileSegment::Transactions => {
                            // Used as ID for validation
                            tx.nonce = *next_tx_num;
                            let tx: TransactionSigned =
                                tx.clone().into_signed(Signature::test_signature()).into();
                            writer.append_transaction(*next_tx_num, &tx).unwrap();
                        }
                        StaticFileSegment::Receipts => {
                            // Used as ID for validation
                            receipt.cumulative_gas_used = *next_tx_num;
                            writer.append_receipt(*next_tx_num, &receipt).unwrap();
                        }
                        StaticFileSegment::TransactionSenders => {
                            // Used as ID for validation
                            let sender = Address::from(U160::from(*next_tx_num));
                            writer.append_transaction_sender(*next_tx_num, &sender).unwrap();
                        }
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
                    .get_segment_provider_for_block(segment, block_range.start, None)
                    .unwrap()
                    .user_header()
                    .tx_range(),
                expected_tx_range
            );
        });

        // Ensure transaction index
        let expected_tx_index = BTreeMap::from([
            (8, SegmentRangeInclusive::new(0, 9)),
            (9, SegmentRangeInclusive::new(20, 29)),
        ]);
        assert_eq!(
            sf_rw.tx_index(segment),
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
        let initial_file_count = files_per_range * file_set_count;

        #[expect(clippy::too_many_arguments)]
        fn prune_and_validate(
            sf_rw: &StaticFileProvider<EthPrimitives>,
            static_dir: impl AsRef<Path>,
            segment: StaticFileSegment,
            prune_count: u64,
            last_block: u64,
            expected_tx_tip: Option<u64>,
            expected_file_count: i32,
            expected_tx_index: BTreeMap<TxNumber, SegmentRangeInclusive>,
        ) -> eyre::Result<()> {
            let mut writer = sf_rw.latest_writer(segment)?;

            // Prune transactions or receipts based on the segment type
            match segment {
                StaticFileSegment::Headers |
                StaticFileSegment::AccountChangeSets |
                StaticFileSegment::StorageChangeSets => {
                    panic!("non tx based segment")
                }
                StaticFileSegment::Transactions => {
                    writer.prune_transactions(prune_count, last_block)?
                }
                StaticFileSegment::Receipts => writer.prune_receipts(prune_count, last_block)?,
                StaticFileSegment::TransactionSenders => {
                    writer.prune_transaction_senders(prune_count, last_block)?
                }
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
                match segment {
                    StaticFileSegment::Headers |
                    StaticFileSegment::AccountChangeSets |
                    StaticFileSegment::StorageChangeSets => {
                        panic!("non tx based segment")
                    }
                    StaticFileSegment::Transactions => assert_eyre(
                        expected_tx_tip,
                        sf_rw.transaction_by_id(id)?.map(|t| t.nonce()),
                        "tx mismatch",
                    )?,
                    StaticFileSegment::Receipts => assert_eyre(
                        expected_tx_tip,
                        sf_rw.receipt(id)?.map(|r| r.cumulative_gas_used),
                        "receipt mismatch",
                    )?,
                    StaticFileSegment::TransactionSenders => assert_eyre(
                        expected_tx_tip,
                        sf_rw
                            .transaction_sender(id)?
                            .map(|s| u64::try_from(U160::from_be_bytes(s.0.into())).unwrap()),
                        "sender mismatch",
                    )?,
                }
            }

            // Ensure the file count has reduced as expected
            assert_eyre(
                count_files_without_lockfile(static_dir)?,
                expected_file_count as usize,
                "file count mismatch",
            )?;

            // Ensure that the inner tx index (max_tx -> block range) is as expected
            assert_eyre(
                sf_rw.tx_index(segment).map(|index| index.iter().map(|(k, v)| (*k, *v)).collect()),
                (!expected_tx_index.is_empty()).then_some(expected_tx_index),
                "tx index mismatch",
            )?;

            Ok(())
        }

        for segment in segments {
            let (static_dir, _) = create_test_static_files_dir();

            let sf_rw = StaticFileProviderBuilder::read_write(&static_dir)
                .with_blocks_per_file(blocks_per_file)
                .build()
                .expect("Failed to build static file provider");

            setup_tx_based_scenario(&sf_rw, segment, blocks_per_file);

            let sf_rw = StaticFileProviderBuilder::read_write(&static_dir)
                .with_blocks_per_file(blocks_per_file)
                .build()
                .expect("Failed to build static file provider");
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
                    BTreeMap::from([(highest_tx - 1, SegmentRangeInclusive::new(0, 9))]),
                ),
                // Case 1: 10..=19 has no txs. There are no txes in the whole block range, but want
                // to unwind to block 9. Ensures that the 20..=29 and 10..=19 files
                // are deleted.
                (
                    0,
                    blocks_per_file - 1,
                    Some(highest_tx - 1),
                    files_per_range,
                    BTreeMap::from([(highest_tx - 1, SegmentRangeInclusive::new(0, 9))]),
                ),
                // Case 2: Prune most txs up to block 1.
                (
                    highest_tx - 1,
                    1,
                    Some(0),
                    files_per_range,
                    BTreeMap::from([(0, SegmentRangeInclusive::new(0, 1))]),
                ),
                // Case 3: Prune remaining tx and ensure that file is not deleted.
                (1, 0, None, files_per_range, BTreeMap::from([])),
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

    /// Returns the number of files in the provided path, excluding ".lock" files.
    fn count_files_without_lockfile(path: impl AsRef<Path>) -> eyre::Result<usize> {
        let is_lockfile = |entry: &fs::DirEntry| {
            entry.path().file_name().map(|name| name == "lock").unwrap_or(false)
        };
        let count = fs::read_dir(path)?
            .filter_map(|entry| entry.ok())
            .filter(|entry| !is_lockfile(entry))
            .count();

        Ok(count)
    }

    #[test]
    fn test_dynamic_size() -> eyre::Result<()> {
        let (static_dir, _) = create_test_static_files_dir();

        {
            let sf_rw: StaticFileProvider<EthPrimitives> =
                StaticFileProviderBuilder::read_write(&static_dir)
                    .with_blocks_per_file(10)
                    .build()?;
            let mut header_writer = sf_rw.latest_writer(StaticFileSegment::Headers)?;

            let mut header = Header::default();
            for num in 0..=15 {
                header.number = num;
                header_writer.append_header(&header, &BlockHash::default()).unwrap();
            }
            header_writer.commit().unwrap();

            assert_eq!(sf_rw.headers_range(0..=15)?.len(), 16);
            assert_eq!(
                sf_rw.expected_block_index(StaticFileSegment::Headers),
                Some(BTreeMap::from([
                    (9, SegmentRangeInclusive::new(0, 9)),
                    (19, SegmentRangeInclusive::new(10, 19))
                ])),
            )
        }

        {
            let sf_rw: StaticFileProvider<EthPrimitives> =
                StaticFileProviderBuilder::read_write(&static_dir)
                    .with_blocks_per_file(5)
                    .build()?;
            let mut header_writer = sf_rw.latest_writer(StaticFileSegment::Headers)?;

            let mut header = Header::default();
            for num in 16..=22 {
                header.number = num;
                header_writer.append_header(&header, &BlockHash::default()).unwrap();
            }
            header_writer.commit().unwrap();

            assert_eq!(sf_rw.headers_range(0..=22)?.len(), 23);
            assert_eq!(
                sf_rw.expected_block_index(StaticFileSegment::Headers),
                Some(BTreeMap::from([
                    (9, SegmentRangeInclusive::new(0, 9)),
                    (19, SegmentRangeInclusive::new(10, 19)),
                    (24, SegmentRangeInclusive::new(20, 24))
                ]))
            )
        }

        {
            let sf_rw: StaticFileProvider<EthPrimitives> =
                StaticFileProviderBuilder::read_write(&static_dir)
                    .with_blocks_per_file(15)
                    .build()?;
            let mut header_writer = sf_rw.latest_writer(StaticFileSegment::Headers)?;

            let mut header = Header::default();
            for num in 23..=40 {
                header.number = num;
                header_writer.append_header(&header, &BlockHash::default()).unwrap();
            }
            header_writer.commit().unwrap();

            assert_eq!(sf_rw.headers_range(0..=40)?.len(), 41);
            assert_eq!(
                sf_rw.expected_block_index(StaticFileSegment::Headers),
                Some(BTreeMap::from([
                    (9, SegmentRangeInclusive::new(0, 9)),
                    (19, SegmentRangeInclusive::new(10, 19)),
                    (24, SegmentRangeInclusive::new(20, 24)),
                    (39, SegmentRangeInclusive::new(25, 39)),
                    (54, SegmentRangeInclusive::new(40, 54))
                ]))
            )
        }

        Ok(())
    }

    #[test]
    fn test_account_changeset_static_files() {
        let (static_dir, _) = create_test_static_files_dir();

        let sf_rw = StaticFileProvider::<EthPrimitives>::read_write(&static_dir)
            .expect("Failed to create static file provider");

        // Helper function to generate test changesets
        fn generate_test_changesets(
            block_num: u64,
            addresses: Vec<Address>,
        ) -> Vec<AccountBeforeTx> {
            addresses
                .into_iter()
                .map(|address| AccountBeforeTx {
                    address,
                    info: Some(Account {
                        nonce: block_num,
                        balance: U256::from(block_num * 1000),
                        bytecode_hash: None,
                    }),
                })
                .collect()
        }

        // Test writing and reading account changesets
        {
            let mut writer = sf_rw.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();

            // Create test data for multiple blocks
            let test_blocks = 10u64;
            let addresses_per_block = 5;

            for block_num in 0..test_blocks {
                // Generate unique addresses for each block
                let addresses: Vec<Address> = (0..addresses_per_block)
                    .map(|i| {
                        let mut addr = Address::ZERO;
                        addr.0[0] = block_num as u8;
                        addr.0[1] = i as u8;
                        addr
                    })
                    .collect();

                let changeset = generate_test_changesets(block_num, addresses.clone());

                writer.append_account_changeset(changeset, block_num).unwrap();
            }

            writer.commit().unwrap();
        }

        // Verify data can be read back correctly
        {
            let provider = sf_rw
                .get_segment_provider_for_block(StaticFileSegment::AccountChangeSets, 5, None)
                .unwrap();

            // Check that the segment header has changeset offsets
            assert!(provider.user_header().changeset_offsets().is_some());
            let offsets = provider.user_header().changeset_offsets().unwrap();
            assert_eq!(offsets.len(), 10); // Should have 10 blocks worth of offsets

            // Verify each block has the expected number of changes
            for (i, offset) in offsets.iter().enumerate() {
                assert_eq!(offset.num_changes(), 5, "Block {} should have 5 changes", i);
            }
        }
    }

    #[test]
    fn test_get_account_before_block() {
        let (static_dir, _) = create_test_static_files_dir();

        let sf_rw = StaticFileProvider::<EthPrimitives>::read_write(&static_dir)
            .expect("Failed to create static file provider");

        // Setup test data
        let test_address = Address::from([1u8; 20]);
        let other_address = Address::from([2u8; 20]);
        let missing_address = Address::from([3u8; 20]);

        // Write changesets for multiple blocks
        {
            let mut writer = sf_rw.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();

            // Block 0: test_address and other_address change
            writer
                .append_account_changeset(
                    vec![
                        AccountBeforeTx {
                            address: test_address,
                            info: None, // Account created
                        },
                        AccountBeforeTx { address: other_address, info: None },
                    ],
                    0,
                )
                .unwrap();

            // Block 1: only other_address changes
            writer
                .append_account_changeset(
                    vec![AccountBeforeTx {
                        address: other_address,
                        info: Some(Account { nonce: 0, balance: U256::ZERO, bytecode_hash: None }),
                    }],
                    1,
                )
                .unwrap();

            // Block 2: test_address changes again
            writer
                .append_account_changeset(
                    vec![AccountBeforeTx {
                        address: test_address,
                        info: Some(Account {
                            nonce: 1,
                            balance: U256::from(1000),
                            bytecode_hash: None,
                        }),
                    }],
                    2,
                )
                .unwrap();

            writer.commit().unwrap();
        }

        // Test get_account_before_block
        {
            // Test retrieving account state before block 0
            let result = sf_rw.get_account_before_block(0, test_address).unwrap();
            assert!(result.is_some());
            let account_before = result.unwrap();
            assert_eq!(account_before.address, test_address);
            assert!(account_before.info.is_none()); // Was created in block 0

            // Test retrieving account state before block 2
            let result = sf_rw.get_account_before_block(2, test_address).unwrap();
            assert!(result.is_some());
            let account_before = result.unwrap();
            assert_eq!(account_before.address, test_address);
            assert!(account_before.info.is_some());
            let info = account_before.info.unwrap();
            assert_eq!(info.nonce, 1);
            assert_eq!(info.balance, U256::from(1000));

            // Test retrieving account that doesn't exist in changeset for block
            let result = sf_rw.get_account_before_block(1, test_address).unwrap();
            assert!(result.is_none()); // test_address didn't change in block 1

            // Test retrieving account that never existed
            let result = sf_rw.get_account_before_block(2, missing_address).unwrap();
            assert!(result.is_none());

            // Test other_address changes
            let result = sf_rw.get_account_before_block(1, other_address).unwrap();
            assert!(result.is_some());
            let account_before = result.unwrap();
            assert_eq!(account_before.address, other_address);
            assert!(account_before.info.is_some());
        }
    }

    #[test]
    fn test_account_changeset_truncation() {
        let (static_dir, _) = create_test_static_files_dir();

        let blocks_per_file = 10;
        let files_per_range = 3;
        let file_set_count = 3;
        let initial_file_count = files_per_range * file_set_count;
        let tip = blocks_per_file * file_set_count - 1;

        // Setup: Create account changesets for multiple blocks
        {
            let sf_rw: StaticFileProvider<EthPrimitives> =
                StaticFileProviderBuilder::read_write(&static_dir)
                    .with_blocks_per_file(blocks_per_file)
                    .build()
                    .expect("failed to create static file provider");

            let mut writer = sf_rw.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();

            for block_num in 0..=tip {
                // Create varying number of changes per block
                let num_changes = ((block_num % 5) + 1) as usize;
                let mut changeset = Vec::with_capacity(num_changes);

                for i in 0..num_changes {
                    let mut address = Address::ZERO;
                    address.0[0] = block_num as u8;
                    address.0[1] = i as u8;

                    changeset.push(AccountBeforeTx {
                        address,
                        info: Some(Account {
                            nonce: block_num,
                            balance: U256::from(block_num * 1000 + i as u64),
                            bytecode_hash: None,
                        }),
                    });
                }

                writer.append_account_changeset(changeset, block_num).unwrap();
            }

            writer.commit().unwrap();
        }

        // Helper function to validate truncation
        fn validate_truncation(
            sf_rw: &StaticFileProvider<EthPrimitives>,
            static_dir: impl AsRef<Path>,
            expected_tip: Option<u64>,
            expected_file_count: u64,
        ) -> eyre::Result<()> {
            // Verify highest block
            let highest_block =
                sf_rw.get_highest_static_file_block(StaticFileSegment::AccountChangeSets);
            assert_eyre(highest_block, expected_tip, "block tip mismatch")?;

            // Verify file count
            assert_eyre(
                count_files_without_lockfile(static_dir)?,
                expected_file_count as usize,
                "file count mismatch",
            )?;

            if let Some(tip) = expected_tip {
                // Verify we can still read data up to the tip
                let provider = sf_rw.get_segment_provider_for_block(
                    StaticFileSegment::AccountChangeSets,
                    tip,
                    None,
                )?;

                // Check offsets are valid
                let offsets = provider.user_header().changeset_offsets();
                assert!(offsets.is_some(), "Should have changeset offsets");
            }

            Ok(())
        }

        // Test truncation scenarios
        let sf_rw = StaticFileProviderBuilder::read_write(&static_dir)
            .with_blocks_per_file(blocks_per_file)
            .build()
            .expect("failed to create static file provider");

        // Re-initialize the index to ensure it knows about the written files
        sf_rw.initialize_index().expect("Failed to initialize index");

        // Case 1: Truncate to block 20 (remove last 9 blocks)
        {
            let mut writer = sf_rw.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();
            writer.prune_account_changesets(20).unwrap();
            writer.commit().unwrap();

            validate_truncation(&sf_rw, &static_dir, Some(20), initial_file_count)
                .expect("Truncation validation failed");
        }

        // Case 2: Truncate to block 9 (should remove 2 files)
        {
            let mut writer = sf_rw.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();
            writer.prune_account_changesets(9).unwrap();
            writer.commit().unwrap();

            validate_truncation(&sf_rw, &static_dir, Some(9), files_per_range)
                .expect("Truncation validation failed");
        }

        // Case 3: Truncate all (should keep block 0)
        {
            let mut writer = sf_rw.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();
            writer.prune_account_changesets(0).unwrap();
            writer.commit().unwrap();

            // AccountChangeSets behaves like tx-based segments and keeps at least block 0
            validate_truncation(&sf_rw, &static_dir, Some(0), files_per_range)
                .expect("Truncation validation failed");
        }
    }

    #[test]
    fn test_changeset_binary_search() {
        let (static_dir, _) = create_test_static_files_dir();

        let sf_rw = StaticFileProvider::<EthPrimitives>::read_write(&static_dir)
            .expect("Failed to create static file provider");

        // Create a block with many account changes to test binary search
        let block_num = 0u64;
        let num_accounts = 100;

        let mut addresses: Vec<Address> = Vec::with_capacity(num_accounts);
        for i in 0..num_accounts {
            let mut addr = Address::ZERO;
            addr.0[0] = (i / 256) as u8;
            addr.0[1] = (i % 256) as u8;
            addresses.push(addr);
        }

        // Write the changeset
        {
            let mut writer = sf_rw.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();

            let changeset: Vec<AccountBeforeTx> = addresses
                .iter()
                .map(|addr| AccountBeforeTx {
                    address: *addr,
                    info: Some(Account {
                        nonce: 1,
                        balance: U256::from(1000),
                        bytecode_hash: None,
                    }),
                })
                .collect();

            writer.append_account_changeset(changeset, block_num).unwrap();
            writer.commit().unwrap();
        }

        // Test binary search for various addresses
        {
            // Test finding first address
            let result = sf_rw.get_account_before_block(block_num, addresses[0]).unwrap();
            assert!(result.is_some());
            assert_eq!(result.unwrap().address, addresses[0]);

            // Test finding last address
            let result =
                sf_rw.get_account_before_block(block_num, addresses[num_accounts - 1]).unwrap();
            assert!(result.is_some());
            assert_eq!(result.unwrap().address, addresses[num_accounts - 1]);

            // Test finding middle addresses
            let mid = num_accounts / 2;
            let result = sf_rw.get_account_before_block(block_num, addresses[mid]).unwrap();
            assert!(result.is_some());
            assert_eq!(result.unwrap().address, addresses[mid]);

            // Test not finding address that doesn't exist
            let mut missing_addr = Address::ZERO;
            missing_addr.0[0] = 255;
            missing_addr.0[1] = 255;
            let result = sf_rw.get_account_before_block(block_num, missing_addr).unwrap();
            assert!(result.is_none());

            // Test multiple lookups for performance
            for i in (0..num_accounts).step_by(10) {
                let result = sf_rw.get_account_before_block(block_num, addresses[i]).unwrap();
                assert!(result.is_some());
                assert_eq!(result.unwrap().address, addresses[i]);
            }
        }
    }

    #[test]
    fn test_storage_changeset_static_files() {
        let (static_dir, _) = create_test_static_files_dir();

        let sf_rw = StaticFileProvider::<EthPrimitives>::read_write(&static_dir)
            .expect("Failed to create static file provider");

        // Test writing and reading storage changesets
        {
            let mut writer = sf_rw.latest_writer(StaticFileSegment::StorageChangeSets).unwrap();

            // Create test data for multiple blocks
            let test_blocks = 10u64;
            let entries_per_block = 5;

            for block_num in 0..test_blocks {
                let changeset = (0..entries_per_block)
                    .map(|i| {
                        let mut addr = Address::ZERO;
                        addr.0[0] = block_num as u8;
                        addr.0[1] = i as u8;
                        StorageBeforeTx {
                            address: addr,
                            key: B256::with_last_byte(i as u8),
                            value: U256::from(block_num * 1000 + i as u64),
                        }
                    })
                    .collect::<Vec<_>>();

                writer.append_storage_changeset(changeset, block_num).unwrap();
            }

            writer.commit().unwrap();
        }

        // Verify data can be read back correctly
        {
            let provider = sf_rw
                .get_segment_provider_for_block(StaticFileSegment::StorageChangeSets, 5, None)
                .unwrap();

            // Check that the segment header has changeset offsets
            assert!(provider.user_header().changeset_offsets().is_some());
            let offsets = provider.user_header().changeset_offsets().unwrap();
            assert_eq!(offsets.len(), 10); // Should have 10 blocks worth of offsets

            // Verify each block has the expected number of changes
            for (i, offset) in offsets.iter().enumerate() {
                assert_eq!(offset.num_changes(), 5, "Block {} should have 5 changes", i);
            }
        }
    }

    #[test]
    fn test_get_storage_before_block() {
        let (static_dir, _) = create_test_static_files_dir();

        let sf_rw = StaticFileProvider::<EthPrimitives>::read_write(&static_dir)
            .expect("Failed to create static file provider");

        let test_address = Address::from([1u8; 20]);
        let other_address = Address::from([2u8; 20]);
        let missing_address = Address::from([3u8; 20]);
        let test_key = B256::with_last_byte(1);
        let other_key = B256::with_last_byte(2);

        // Write changesets for multiple blocks
        {
            let mut writer = sf_rw.latest_writer(StaticFileSegment::StorageChangeSets).unwrap();

            // Block 0: test_address and other_address change
            writer
                .append_storage_changeset(
                    vec![
                        StorageBeforeTx { address: test_address, key: test_key, value: U256::ZERO },
                        StorageBeforeTx {
                            address: other_address,
                            key: other_key,
                            value: U256::from(5),
                        },
                    ],
                    0,
                )
                .unwrap();

            // Block 1: only other_address changes
            writer
                .append_storage_changeset(
                    vec![StorageBeforeTx {
                        address: other_address,
                        key: other_key,
                        value: U256::from(7),
                    }],
                    1,
                )
                .unwrap();

            // Block 2: test_address changes again
            writer
                .append_storage_changeset(
                    vec![StorageBeforeTx {
                        address: test_address,
                        key: test_key,
                        value: U256::from(9),
                    }],
                    2,
                )
                .unwrap();

            writer.commit().unwrap();
        }

        // Test get_storage_before_block
        {
            let result = sf_rw.get_storage_before_block(0, test_address, test_key).unwrap();
            assert!(result.is_some());
            let entry = result.unwrap();
            assert_eq!(entry.key, test_key);
            assert_eq!(entry.value, U256::ZERO);

            let result = sf_rw.get_storage_before_block(2, test_address, test_key).unwrap();
            assert!(result.is_some());
            let entry = result.unwrap();
            assert_eq!(entry.key, test_key);
            assert_eq!(entry.value, U256::from(9));

            let result = sf_rw.get_storage_before_block(1, test_address, test_key).unwrap();
            assert!(result.is_none());

            let result = sf_rw.get_storage_before_block(2, missing_address, test_key).unwrap();
            assert!(result.is_none());

            let result = sf_rw.get_storage_before_block(1, other_address, other_key).unwrap();
            assert!(result.is_some());
            let entry = result.unwrap();
            assert_eq!(entry.key, other_key);
        }
    }

    #[test]
    fn test_storage_changeset_truncation() {
        let (static_dir, _) = create_test_static_files_dir();

        let blocks_per_file = 10;
        let files_per_range = 3;
        let file_set_count = 3;
        let initial_file_count = files_per_range * file_set_count;
        let tip = blocks_per_file * file_set_count - 1;

        // Setup: Create storage changesets for multiple blocks
        {
            let sf_rw: StaticFileProvider<EthPrimitives> =
                StaticFileProviderBuilder::read_write(&static_dir)
                    .with_blocks_per_file(blocks_per_file)
                    .build()
                    .expect("failed to create static file provider");

            let mut writer = sf_rw.latest_writer(StaticFileSegment::StorageChangeSets).unwrap();

            for block_num in 0..=tip {
                let num_changes = ((block_num % 5) + 1) as usize;
                let mut changeset = Vec::with_capacity(num_changes);

                for i in 0..num_changes {
                    let mut address = Address::ZERO;
                    address.0[0] = block_num as u8;
                    address.0[1] = i as u8;

                    changeset.push(StorageBeforeTx {
                        address,
                        key: B256::with_last_byte(i as u8),
                        value: U256::from(block_num * 1000 + i as u64),
                    });
                }

                writer.append_storage_changeset(changeset, block_num).unwrap();
            }

            writer.commit().unwrap();
        }

        fn validate_truncation(
            sf_rw: &StaticFileProvider<EthPrimitives>,
            static_dir: impl AsRef<Path>,
            expected_tip: Option<u64>,
            expected_file_count: u64,
        ) -> eyre::Result<()> {
            let highest_block =
                sf_rw.get_highest_static_file_block(StaticFileSegment::StorageChangeSets);
            assert_eyre(highest_block, expected_tip, "block tip mismatch")?;

            assert_eyre(
                count_files_without_lockfile(static_dir)?,
                expected_file_count as usize,
                "file count mismatch",
            )?;

            if let Some(tip) = expected_tip {
                let provider = sf_rw.get_segment_provider_for_block(
                    StaticFileSegment::StorageChangeSets,
                    tip,
                    None,
                )?;
                let offsets = provider.user_header().changeset_offsets();
                assert!(offsets.is_some(), "Should have changeset offsets");
            }

            Ok(())
        }

        let sf_rw = StaticFileProviderBuilder::read_write(&static_dir)
            .with_blocks_per_file(blocks_per_file)
            .build()
            .expect("failed to create static file provider");

        sf_rw.initialize_index().expect("Failed to initialize index");

        // Case 1: Truncate to block 20
        {
            let mut writer = sf_rw.latest_writer(StaticFileSegment::StorageChangeSets).unwrap();
            writer.prune_storage_changesets(20).unwrap();
            writer.commit().unwrap();

            validate_truncation(&sf_rw, &static_dir, Some(20), initial_file_count)
                .expect("Truncation validation failed");
        }

        // Case 2: Truncate to block 9
        {
            let mut writer = sf_rw.latest_writer(StaticFileSegment::StorageChangeSets).unwrap();
            writer.prune_storage_changesets(9).unwrap();
            writer.commit().unwrap();

            validate_truncation(&sf_rw, &static_dir, Some(9), files_per_range)
                .expect("Truncation validation failed");
        }

        // Case 3: Truncate all (should keep block 0)
        {
            let mut writer = sf_rw.latest_writer(StaticFileSegment::StorageChangeSets).unwrap();
            writer.prune_storage_changesets(0).unwrap();
            writer.commit().unwrap();

            validate_truncation(&sf_rw, &static_dir, Some(0), files_per_range)
                .expect("Truncation validation failed");
        }
    }

    #[test]
    fn test_storage_changeset_binary_search() {
        let (static_dir, _) = create_test_static_files_dir();

        let sf_rw = StaticFileProvider::<EthPrimitives>::read_write(&static_dir)
            .expect("Failed to create static file provider");

        let block_num = 0u64;
        let num_slots = 100;
        let address = Address::from([4u8; 20]);

        let mut keys: Vec<B256> = Vec::with_capacity(num_slots);
        for i in 0..num_slots {
            keys.push(B256::with_last_byte(i as u8));
        }

        {
            let mut writer = sf_rw.latest_writer(StaticFileSegment::StorageChangeSets).unwrap();
            let changeset = keys
                .iter()
                .enumerate()
                .map(|(i, key)| StorageBeforeTx { address, key: *key, value: U256::from(i as u64) })
                .collect::<Vec<_>>();

            writer.append_storage_changeset(changeset, block_num).unwrap();
            writer.commit().unwrap();
        }

        {
            let result = sf_rw.get_storage_before_block(block_num, address, keys[0]).unwrap();
            assert!(result.is_some());
            let entry = result.unwrap();
            assert_eq!(entry.key, keys[0]);
            assert_eq!(entry.value, U256::from(0));

            let result =
                sf_rw.get_storage_before_block(block_num, address, keys[num_slots - 1]).unwrap();
            assert!(result.is_some());
            let entry = result.unwrap();
            assert_eq!(entry.key, keys[num_slots - 1]);

            let mid = num_slots / 2;
            let result = sf_rw.get_storage_before_block(block_num, address, keys[mid]).unwrap();
            assert!(result.is_some());
            let entry = result.unwrap();
            assert_eq!(entry.key, keys[mid]);

            let missing_key = B256::with_last_byte(255);
            let result = sf_rw.get_storage_before_block(block_num, address, missing_key).unwrap();
            assert!(result.is_none());

            for i in (0..num_slots).step_by(10) {
                let result = sf_rw.get_storage_before_block(block_num, address, keys[i]).unwrap();
                assert!(result.is_some());
                assert_eq!(result.unwrap().key, keys[i]);
            }
        }
    }
}

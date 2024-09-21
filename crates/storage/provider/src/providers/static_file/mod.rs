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
    use alloy_primitives::{B256, U256};
    use rand::seq::SliceRandom;
    use reth_db::{
        test_utils::create_test_static_files_dir, CanonicalHeaders, HeaderNumbers,
        HeaderTerminalDifficulties, Headers,
    };
    use reth_db_api::transaction::DbTxMut;
    use reth_primitives::{
        static_file::{find_fixed_range, DEFAULT_BLOCKS_PER_STATIC_FILE},
        BlockHash, Header,
    };
    use reth_testing_utils::generators::{self, random_header_range};
    use std::{fs, path::Path};

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
        ) {
            writer.prune_headers(prune_count).unwrap();
            writer.commit().unwrap();

            // Validate the highest block after pruning
            assert_eq!(
                sf_rw.get_highest_static_file_block(StaticFileSegment::Headers),
                expected_tip
            );

            // Validate the number of files remaining in the directory
            assert_eq!(fs::read_dir(static_dir).unwrap().count(), expected_file_count as usize);
        }

        // [ Test Cases ]
        type PruneCount = u64;
        type ExpectedTip = u64;
        type ExpectedFileCount = u64;
        let mut tmp_tip = tip;
        let test_cases: Vec<(PruneCount, Option<ExpectedTip>, ExpectedFileCount)> = vec![
            // Case 1: Pruning 1 header
            {
                tmp_tip -= 1;
                (1, Some(tmp_tip), initial_file_count)
            },
            // Case 2: Pruning remaining rows from file should result in its deletion
            {
                tmp_tip -= blocks_per_file - 1;
                (blocks_per_file - 1, Some(tmp_tip), initial_file_count - files_per_range)
            },
            // Case 3: Pruning more headers than a single file has (tip reduced by
            // blocks_per_file + 1) should result in a file set deletion
            {
                tmp_tip -= blocks_per_file + 1;
                (blocks_per_file + 1, Some(tmp_tip), initial_file_count - files_per_range * 2)
            },
            // Case 4: Pruning all remaining headers from the file except the genesis header
            {
                (
                    tmp_tip,
                    Some(0),             // Only genesis block remains
                    files_per_range + 1, // The file set with block 0 should remain
                )
            },
            // Case 5: Pruning the genesis header (should not delete the file set with block 0)
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

            for (prune_count, expected_tip, expected_file_count) in test_cases {
                prune_and_validate(
                    &mut header_writer,
                    &sf_rw,
                    &static_dir,
                    prune_count,
                    expected_tip,
                    expected_file_count,
                );
            }
        }
    }
}

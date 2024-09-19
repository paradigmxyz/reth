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
    use reth_db::{CanonicalHeaders, HeaderNumbers, HeaderTerminalDifficulties, Headers};
    use reth_db_api::transaction::DbTxMut;
    use reth_primitives::static_file::{find_fixed_range, DEFAULT_BLOCKS_PER_STATIC_FILE};
    use reth_testing_utils::generators::{self, random_header_range};

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
}

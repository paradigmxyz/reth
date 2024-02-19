mod manager;
pub use manager::SnapshotProvider;

mod jar;
pub use jar::SnapshotJarProvider;

use reth_interfaces::provider::ProviderResult;
use reth_nippy_jar::NippyJar;
use reth_primitives::{snapshot::SegmentHeader, SnapshotSegment};
use std::{ops::Deref, sync::Arc};

/// Alias type for each specific `NippyJar`.
type LoadedJarRef<'a> = dashmap::mapref::one::Ref<'a, (u64, SnapshotSegment), LoadedJar>;

/// Helper type to reuse an associated snapshot mmap handle on created cursors.
#[derive(Debug)]
pub struct LoadedJar {
    jar: NippyJar<SegmentHeader>,
    mmap_handle: Arc<reth_nippy_jar::DataReader>,
}

impl LoadedJar {
    fn new(jar: NippyJar<SegmentHeader>) -> ProviderResult<Self> {
        let mmap_handle = Arc::new(jar.open_data_reader()?);
        Ok(Self { jar, mmap_handle })
    }

    /// Returns a clone of the mmap handle that can be used to instantiate a cursor.
    fn mmap_handle(&self) -> Arc<reth_nippy_jar::DataReader> {
        self.mmap_handle.clone()
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
    use rand::seq::SliceRandom;
    use reth_db::{
        cursor::DbCursorRO,
        snapshot::create_snapshot_T1_T2_T3,
        transaction::{DbTx, DbTxMut},
        CanonicalHeaders, HeaderNumbers, HeaderTD, Headers, RawTable,
    };
    use reth_interfaces::test_utils::generators::{self, random_header_range};
    use reth_primitives::{BlockNumber, B256, U256};

    #[test]
    fn test_snap() {
        // Ranges
        let row_count = 100u64;
        let range = 0..=(row_count - 1);
        let segment_header =
            SegmentHeader::new(range.clone(), range.clone(), SnapshotSegment::Headers);

        // Data sources
        let factory = create_test_provider_factory();
        let snap_path = tempfile::tempdir().unwrap();
        let snap_file = snap_path.path().join(SnapshotSegment::Headers.filename(&range, &range));

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
            tx.put::<HeaderTD>(header.number, td.into()).unwrap();
            tx.put::<HeaderNumbers>(hash, header.number).unwrap();
        }
        provider_rw.commit().unwrap();

        // Create Snapshot
        {
            let with_compression = true;
            let with_filter = true;

            let mut nippy_jar = NippyJar::new(3, snap_file.as_path(), segment_header);

            if with_compression {
                nippy_jar = nippy_jar.with_zstd(false, 0);
            }

            if with_filter {
                nippy_jar = nippy_jar.with_cuckoo_filter(row_count as usize + 10).with_fmph();
            }

            let provider = factory.provider().unwrap();
            let tx = provider.tx_ref();

            // Hacky type inference. TODO fix
            let mut none_vec = Some(vec![vec![vec![0u8]].into_iter()]);
            let _ = none_vec.take();

            // Generate list of hashes for filters & PHF
            let mut cursor = tx.cursor_read::<RawTable<CanonicalHeaders>>().unwrap();
            let hashes = cursor
                .walk(None)
                .unwrap()
                .map(|row| row.map(|(_key, value)| value.into_value()).map_err(|e| e.into()));

            create_snapshot_T1_T2_T3::<
                Headers,
                HeaderTD,
                CanonicalHeaders,
                BlockNumber,
                SegmentHeader,
            >(
                tx, range, None, none_vec, Some(hashes), row_count as usize, &mut nippy_jar
            )
            .unwrap();
        }

        // Use providers to query Header data and compare if it matches
        {
            let db_provider = factory.provider().unwrap();
            let manager = SnapshotProvider::new(snap_path.path()).unwrap().with_filters();
            let jar_provider = manager
                .get_segment_provider_from_block(SnapshotSegment::Headers, 0, Some(&snap_file))
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

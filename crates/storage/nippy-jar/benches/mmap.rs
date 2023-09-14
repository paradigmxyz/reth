use criterion::{
    criterion_group, criterion_main, measurement::WallTime, BenchmarkGroup, Criterion,
};
use rand::{seq::SliceRandom, Rng};
use reth_interfaces::test_utils::generators::random_block_range;
use reth_nippy_jar::mmap::Mmap;
use reth_primitives::{SealedBlock, H256};
use reth_rlp::Encodable;
use std::{
    fs::{File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::Path,
    time::Duration,
};
use tempfile::tempdir;

trait SnapshotReader: Sized {
    /// Returns (reader, offsets).
    fn setup<R: Rng>(
        rng: &mut R,
        dir: &Path,
        data: &[SealedBlock],
    ) -> std::io::Result<(Self, Vec<(usize, usize)>)>;

    fn read(&mut self, offset: usize, len: usize) -> std::io::Result<Vec<u8>>;

    fn read_multiple(&mut self, offsets: &[(usize, usize)]) -> std::io::Result<Vec<Vec<u8>>>;
}

pub fn snapshots_reads(c: &mut Criterion) {
    let mut group = c.benchmark_group("Snapshot Reads");
    group.measurement_time(Duration::from_secs(10));
    group.warm_up_time(Duration::from_nanos(1));

    let mut rng = rand::thread_rng();
    let dir = tempdir().expect("failed to create tempdir");
    let dir_path = dir.path();
    let window_range = 5;

    let num_snapshots = 10;
    for blocks_per_snapshot in [10, 100, 1000] {
        let test_data = generate_test_data(&mut rng, num_snapshots, blocks_per_snapshot);

        let bench_description = |reader| {
            format!("snapshots: {num_snapshots} | blocks per snapshot: {blocks_per_snapshot} | {reader}")
        };

        use implementations::*;

        reader_bench::<_, FileReader>(
            &mut group,
            &mut rng,
            &dir_path,
            &bench_description("FileReader"),
            &test_data,
            window_range,
        );

        reader_bench::<_, MdbxReader>(
            &mut group,
            &mut rng,
            &dir_path,
            &bench_description("MdbxReader"),
            &test_data,
            window_range,
        );

        reader_bench::<_, MmapReader>(
            &mut group,
            &mut rng,
            &dir_path,
            &bench_description("MmapReader"),
            &test_data,
            window_range,
        );
    }
}

fn generate_test_data<R: Rng>(
    rng: &mut R,
    num_snapshots: usize,
    blocks_per_snapshot: usize,
) -> Vec<Vec<SealedBlock>> {
    let mut data = Vec::with_capacity(num_snapshots);
    for i in 0..num_snapshots {
        let range_start = (i * num_snapshots) as u64;
        let blocks = random_block_range(
            rng,
            range_start..=range_start + blocks_per_snapshot as u64 - 1,
            H256::random(),
            0..6,
        );
        data.push(blocks);
    }

    data
}

fn reader_bench<R: Rng, T: SnapshotReader>(
    group: &mut BenchmarkGroup<WallTime>,
    rng: &mut R,
    dir: &Path,
    description: &str,
    test_data: &[Vec<SealedBlock>],
    window_range: usize,
) {
    let group_id = |ty| format!("{ty} | {description}");

    let setup = |rng: &mut R| {
        let mut readers = Vec::with_capacity(test_data.len());
        let mut all_offsets = Vec::with_capacity(test_data.len());
        for blocks in test_data {
            let (reader, offsets) = T::setup(rng, dir, &blocks).expect("setup successful");
            readers.push(reader);
            all_offsets.push(offsets);
        }

        (readers, all_offsets)
    };

    group.bench_function(group_id("single seq reads"), |b| {
        b.iter_with_setup(
            || setup(rng),
            |(readers, offsets)| {
                {
                    for (mut reader, offsets) in readers.into_iter().zip(offsets) {
                        for (offset, len) in offsets {
                            reader.read(offset, len).unwrap();
                        }
                    }
                };
                std::hint::black_box(());
            },
        );
    });

    group.bench_function(group_id("multiple seq reads"), |b| {
        b.iter_with_setup(
            || setup(rng),
            |(readers, offsets)| {
                {
                    for (mut reader, offsets) in readers.into_iter().zip(offsets) {
                        for offsets_window in offsets.windows(window_range) {
                            reader.read_multiple(offsets_window).unwrap();
                        }
                    }
                };
                std::hint::black_box(());
            },
        );
    });

    group.bench_function(group_id("random reads"), |b| {
        b.iter_with_setup(
            || {
                let (readers, mut all_offsets) = setup(rng);

                for offsets in all_offsets.iter_mut() {
                    offsets.shuffle(rng);
                }

                (readers, all_offsets)
            },
            |(readers, shuffled_offsets)| {
                {
                    for (mut reader, offsets) in readers.into_iter().zip(shuffled_offsets) {
                        for (offset, len) in offsets {
                            reader.read(offset, len).unwrap();
                        }
                    }
                };
                std::hint::black_box(());
            },
        )
    });
}

criterion_group!(mmap_reads, snapshots_reads);
criterion_main!(mmap_reads);

mod implementations {
    use super::*;
    use reth_db::{
        cursor::DbCursorRO,
        database::Database,
        mdbx::{DatabaseFlags, EnvKind},
        table::Table,
        transaction::{DbTx, DbTxMut},
        DatabaseEnv,
    };
    use reth_primitives::BlockNumber;
    use std::{
        io::{Error, ErrorKind, Read, Result},
        sync::Arc,
    };

    fn create_file_with_rlp_blocks<R: Rng>(
        rng: &mut R,
        dir: &Path,
        data: &[SealedBlock],
    ) -> Result<(File, Vec<(usize, usize)>)> {
        let filename_seed: u64 = rng.gen();
        let filename = format!("snapshot-{}-{}", data.len(), filename_seed);

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .append(true)
            .open(dir.join(filename))?;

        let mut offset = 0;
        let mut offsets = Vec::with_capacity(data.len());
        for block in data {
            let mut buf = Vec::new();
            SealedBlock::encode(&block, &mut buf);
            let len = file.write(&buf)?;
            offsets.push((offset, len));
            offset += len;
        }

        file.seek(SeekFrom::Start(0))?;

        Ok((file, offsets))
    }

    /// This loads whole file in memory on each read. Never use it pls.
    pub struct FileReader {
        file: File,
    }

    impl SnapshotReader for FileReader {
        fn setup<R: Rng>(
            rng: &mut R,
            dir: &Path,
            data: &[SealedBlock],
        ) -> Result<(Self, Vec<(usize, usize)>)> {
            let (file, offsets) = create_file_with_rlp_blocks(rng, dir, data)?;
            Ok((Self { file }, offsets))
        }

        fn read(&mut self, offset: usize, len: usize) -> Result<Vec<u8>> {
            self.file.seek(SeekFrom::Start(offset as u64))?;
            let mut buf = vec![0; len];
            self.file.read_exact(&mut buf)?;
            Ok(buf)
        }

        fn read_multiple(&mut self, offsets: &[(usize, usize)]) -> Result<Vec<Vec<u8>>> {
            let mut results = Vec::with_capacity(offsets.len());

            if offsets.is_empty() {
                return Ok(results)
            }

            let (start, end) = offsets
                .first()
                .map(|(offset, _)| *offset)
                .zip(offsets.last().map(|(offset, len)| offset + len))
                .unwrap();

            self.file.seek(SeekFrom::Start(start as u64))?;

            let mut buf = vec![0; end - start];
            self.file.read_exact(&mut buf)?;

            for (offset, len) in offsets {
                let start_at = *offset - start;
                results.push(buf[start_at..start_at + *len].to_vec());
            }

            Ok(results)
        }
    }

    pub struct MmapReader {
        #[allow(dead_code)]
        file: File,
        mmap: Mmap,
    }

    impl SnapshotReader for MmapReader {
        fn setup<R: Rng>(
            rng: &mut R,
            dir: &Path,
            data: &[SealedBlock],
        ) -> Result<(Self, Vec<(usize, usize)>)> {
            let (file, offsets) = create_file_with_rlp_blocks(rng, dir, data)?;
            let mmap = unsafe { Mmap::map(&file).unwrap() };
            Ok((Self { file, mmap }, offsets))
        }

        fn read(&mut self, offset: usize, len: usize) -> Result<Vec<u8>> {
            Ok(self.mmap[offset..offset + len].to_vec())
        }

        fn read_multiple(&mut self, offsets: &[(usize, usize)]) -> Result<Vec<Vec<u8>>> {
            let mut result = Vec::with_capacity(offsets.len());
            for (offset, len) in offsets {
                result.push(self.mmap[*offset..*offset + *len].to_vec());
            }

            Ok(result)
        }
    }

    // De facto this is a `SealedBlocks` table.
    // It is aliased as a different one to bypass some checks.
    #[derive(Debug)]
    pub struct BlockBodyIndices;

    impl Table for BlockBodyIndices {
        const NAME: &'static str = BlockBodyIndices::const_name();
        type Key = BlockNumber;
        type Value = Vec<u8>;
    }

    impl BlockBodyIndices {
        /// Return BlockBodyIndices as it is present inside the database.
        pub const fn const_name() -> &'static str {
            stringify!(BlockBodyIndices)
        }
    }

    pub struct MdbxReader {
        db: Arc<DatabaseEnv>,
    }

    impl SnapshotReader for MdbxReader {
        fn setup<R: Rng>(
            rng: &mut R,
            dir: &Path,
            data: &[SealedBlock],
        ) -> Result<(Self, Vec<(usize, usize)>)> {
            let db_dir_seed: u64 = rng.gen();
            let db_path = dir.join(format!("{db_dir_seed}"));
            std::fs::create_dir_all(&db_path)?;
            let db = Arc::new(DatabaseEnv::open(&db_path, EnvKind::RW, None).unwrap());

            // Create the table.
            let tx = db.begin_rw_txn().unwrap();
            tx.create_db(Some(BlockBodyIndices::NAME), DatabaseFlags::default()).unwrap();
            tx.commit().unwrap();

            // Use block numbers as offsets.
            let tx = db.tx_mut().unwrap();
            let mut offsets = Vec::with_capacity(data.len());
            for block in data {
                let mut buf = Vec::new();
                SealedBlock::encode(&block, &mut buf);
                tx.put::<BlockBodyIndices>(block.number, buf).unwrap();
                offsets.push((block.number as usize, 0));
            }

            tx.commit().unwrap();

            Ok((MdbxReader { db }, offsets))
        }

        fn read(&mut self, offset: usize, _len: usize) -> Result<Vec<u8>> {
            let tx = self.db.tx().unwrap();
            tx.get::<BlockBodyIndices>(offset as u64).unwrap().ok_or(ErrorKind::InvalidData.into())
        }

        fn read_multiple(&mut self, offsets: &[(usize, usize)]) -> Result<Vec<Vec<u8>>> {
            let tx = self.db.tx().unwrap();
            let mut cursor = tx.cursor_read::<BlockBodyIndices>().unwrap();

            let mut results = Vec::with_capacity(offsets.len());
            for (offset, _) in offsets {
                let (_, block) = cursor
                    .seek_exact(*offset as u64)
                    .unwrap()
                    .ok_or(<ErrorKind as Into<Error>>::into(ErrorKind::InvalidData))?;
                results.push(block);
            }

            Ok(results)
        }
    }
}

use criterion::{
    criterion_group, criterion_main, measurement::WallTime, BenchmarkGroup, Criterion,
};
use proptest::{
    prelude::*,
    strategy::{Strategy, ValueTree},
    test_runner::TestRunner,
};
use rand::seq::SliceRandom;
use reth_nippy_jar::mmap::Mmap;
use reth_primitives::SealedBlock;
use reth_rlp::Encodable;
use std::{
    fs::{File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::Path,
    time::Duration,
};
use tempfile::tempdir;

trait SnapshotReader<'a> {
    fn new(file: &'a File) -> Self;

    // Should return
    fn read(&mut self, offset: usize, len: usize) -> std::io::Result<Vec<u8>>;

    fn read_multiple(&mut self, offsets: &[(usize, usize)]) -> std::io::Result<Vec<Vec<u8>>>;
}

pub fn snapshots_reads(c: &mut Criterion) {
    let mut group = c.benchmark_group("Snapshot Reads");
    group.measurement_time(Duration::from_secs(10));

    let dir = tempdir().expect("failed to create tempdir");
    let range = 5;

    let num_snapshots = 10;
    for blocks_per_snapshot in [10, 100, 1000] {
        let test_data = generate_test_data(dir.path(), num_snapshots, blocks_per_snapshot).unwrap();

        let bench_description = |reader| {
            format!("snapshots: {num_snapshots} | blocks per snapshot: {blocks_per_snapshot} | {reader}")
        };

        use implementations::*;

        reader_bench::<FileReader>(&mut group, &bench_description("FileReader"), &test_data, range);

        reader_bench::<MmapReader>(&mut group, &bench_description("MmapReader"), &test_data, range);
    }
}

fn generate_test_data(
    dir: &Path,
    num_snapshots: usize,
    blocks_per_snapshot: usize,
) -> std::io::Result<Vec<(File, Vec<(usize, usize)>)>> {
    let config = ProptestConfig::default();
    let mut runner = TestRunner::new(config);

    let mut data = Vec::with_capacity(num_snapshots);
    for i in 0..num_snapshots {
        let blocks: Vec<SealedBlock> =
            prop::collection::vec(any::<SealedBlock>(), blocks_per_snapshot)
                .new_tree(&mut runner)
                .unwrap()
                .current();

        let filename = format!("snapshot-{num_snapshots}-{blocks_per_snapshot}-{i}");
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .append(true)
            .open(dir.join(filename))?;

        let mut offset = 0;
        let mut offsets = Vec::with_capacity(blocks.len());
        for block in &blocks {
            let mut buf = Vec::new();
            SealedBlock::encode(&block, &mut buf);
            let len = file.write(&buf)?;
            offsets.push((offset, len));
            offset += len;
        }

        file.seek(SeekFrom::Start(0))?;

        data.push((file, offsets));
    }

    Ok(data)
}

fn reader_bench<'a, T: SnapshotReader<'a>>(
    group: &mut BenchmarkGroup<WallTime>,
    description: &str,
    test_data: &'a [(File, Vec<(usize, usize)>)],
    range: usize,
) {
    let group_id = |ty| format!("{ty} | {description}");

    let setup = || {
        let mut readers = Vec::with_capacity(test_data.len());
        for (file, _) in test_data {
            readers.push(T::new(file));
        }
        readers
    };

    group.bench_function(group_id("single seq reads"), |b| {
        b.iter_with_setup(setup, |readers| {
            {
                for (mut reader, (_, offsets)) in readers.into_iter().zip(test_data) {
                    for (offset, len) in offsets {
                        reader.read(*offset, *len).unwrap();
                    }
                }
            };
            std::hint::black_box(());
        });
    });

    group.bench_function(group_id("multiple seq reads"), |b| {
        b.iter_with_setup(setup, |readers| {
            {
                for (mut reader, (_, offsets)) in readers.into_iter().zip(test_data) {
                    for offsets_window in offsets.windows(range) {
                        reader.read_multiple(offsets_window).unwrap();
                    }
                }
            };
            std::hint::black_box(());
        });
    });

    group.bench_function(group_id("random reads"), |b| {
        b.iter_with_setup(
            || {
                let readers = setup();

                let mut rng = rand::thread_rng();
                let mut shuffled_offsets = Vec::with_capacity(test_data.len());
                for (_, offsets) in test_data {
                    let mut offsets = offsets.clone();
                    offsets.shuffle(&mut rng);
                    shuffled_offsets.push(offsets);
                }

                (readers, shuffled_offsets)
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
    use std::io::{Read, Result};

    /// This loads whole file in memory on each read. Never use it pls.
    pub struct FileReader<'a> {
        file: &'a File,
    }

    impl<'a> SnapshotReader<'a> for FileReader<'a> {
        fn new(file: &'a File) -> Self {
            Self { file }
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
        mmap: Mmap,
    }

    impl<'a> SnapshotReader<'a> for MmapReader {
        fn new(file: &'a File) -> Self {
            Self { mmap: unsafe { Mmap::map(file).unwrap() } }
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
}

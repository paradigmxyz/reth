//! Benchmarks for reading committed changeset offsets from a static file sidecar.

#[cfg(unix)]
mod unix {
    use criterion::{BenchmarkId, Criterion, Throughput};
    use reth_static_file_types::{
        ChangesetOffset, ChangesetOffsetReader, ChangesetOffsetWriter,
        DEFAULT_BLOCKS_PER_STATIC_FILE,
    };
    use tempfile::tempdir;

    fn read_ranges(c: &mut Criterion) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("changesets.csoff");
        let mut writer = ChangesetOffsetWriter::new(&path, 0).unwrap();
        for block in 0..DEFAULT_BLOCKS_PER_STATIC_FILE {
            writer.append(&ChangesetOffset::new(block * 3, 3)).unwrap();
        }
        writer.sync().unwrap();
        let reader = ChangesetOffsetReader::new(path, writer.len()).unwrap();

        let mut group = c.benchmark_group("changeset_offsets/get_range");
        for count in [1, 8, 512, 4097, DEFAULT_BLOCKS_PER_STATIC_FILE] {
            let start = (reader.len() - count) / 2;
            group.throughput(Throughput::Elements(count));
            group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, &count| {
                b.iter(|| reader.get_range(start, start + count).unwrap())
            });
        }
        group.finish();
    }

    criterion::criterion_group!(benches, read_ranges);
}

#[cfg(unix)]
criterion::criterion_main!(unix::benches);

#[cfg(not(unix))]
fn main() {}

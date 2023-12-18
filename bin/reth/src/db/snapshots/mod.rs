use clap::{builder::RangedU64ValueParser, Parser};
use human_bytes::human_bytes;
use itertools::Itertools;
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use reth_db::{database::Database, open_db_read_only, DatabaseEnv};
use reth_interfaces::db::LogLevel;
use reth_nippy_jar::{NippyJar, NippyJarCursor};
use reth_primitives::{
    snapshot::{Compression, Filters, InclusionFilter, PerfectHashingFunction, SegmentHeader},
    BlockNumber, ChainSpec, SnapshotSegment,
};
use reth_provider::{BlockNumReader, ProviderFactory, TransactionsProviderExt};
use reth_snapshot::{segments as snap_segments, segments::Segment};
use std::{
    ops::RangeInclusive,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

mod bench;
mod headers;
mod receipts;
mod transactions;

#[derive(Parser, Debug)]
/// Arguments for the `reth db snapshot` command.
pub struct Command {
    /// Snapshot segments to generate.
    segments: Vec<SnapshotSegment>,

    /// Starting block for the snapshot.
    #[arg(long, short, default_value = "0")]
    from: BlockNumber,

    /// Number of blocks in the snapshot.
    #[arg(long, short, default_value = "500000")]
    block_interval: u64,

    /// Sets the number of snapshots built in parallel. Note: Each parallel build is
    /// memory-intensive.
    #[arg(
        long, short,
        default_value = "1",
        value_parser = RangedU64ValueParser::<u64>::new().range(1..)
    )]
    parallel: u64,

    /// Flag to skip snapshot creation and print snapshot files stats.
    #[arg(long, default_value = "false")]
    only_stats: bool,

    /// Flag to enable database-to-snapshot benchmarking.
    #[arg(long, default_value = "false")]
    bench: bool,

    /// Flag to skip snapshot creation and only run benchmarks on existing snapshots.
    #[arg(long, default_value = "false")]
    only_bench: bool,

    /// Compression algorithms to use.
    #[arg(long, short, value_delimiter = ',', default_value = "uncompressed")]
    compression: Vec<Compression>,

    /// Flag to enable inclusion list filters and PHFs.
    #[arg(long, default_value = "false")]
    with_filters: bool,

    /// Specifies the perfect hashing function to use.
    #[arg(long, value_delimiter = ',', default_value_if("with_filters", "true", "fmph"))]
    phf: Vec<PerfectHashingFunction>,
}

impl Command {
    /// Execute `db snapshot` command
    pub fn execute(
        self,
        db_path: &Path,
        log_level: Option<LogLevel>,
        chain: Arc<ChainSpec>,
    ) -> eyre::Result<()> {
        let all_combinations =
            self.segments.iter().cartesian_product(self.compression.iter()).cartesian_product(
                if self.phf.is_empty() {
                    vec![None]
                } else {
                    self.phf.iter().copied().map(Some).collect::<Vec<_>>()
                },
            );

        {
            let db = open_db_read_only(db_path, None)?;
            let factory = Arc::new(ProviderFactory::new(db, chain.clone()));

            if !self.only_bench {
                for ((mode, compression), phf) in all_combinations.clone() {
                    let filters = if let Some(phf) = self.with_filters.then_some(phf).flatten() {
                        Filters::WithFilters(InclusionFilter::Cuckoo, phf)
                    } else {
                        Filters::WithoutFilters
                    };

                    match mode {
                        SnapshotSegment::Headers => self.generate_snapshot::<DatabaseEnv>(
                            factory.clone(),
                            snap_segments::Headers::new(*compression, filters),
                        )?,
                        SnapshotSegment::Transactions => self.generate_snapshot::<DatabaseEnv>(
                            factory.clone(),
                            snap_segments::Transactions::new(*compression, filters),
                        )?,
                        SnapshotSegment::Receipts => self.generate_snapshot::<DatabaseEnv>(
                            factory.clone(),
                            snap_segments::Receipts::new(*compression, filters),
                        )?,
                    }
                }
            }
        }

        if self.only_bench || self.bench {
            for ((mode, compression), phf) in all_combinations.clone() {
                match mode {
                    SnapshotSegment::Headers => self.bench_headers_snapshot(
                        db_path,
                        log_level,
                        chain.clone(),
                        *compression,
                        InclusionFilter::Cuckoo,
                        phf,
                    )?,
                    SnapshotSegment::Transactions => self.bench_transactions_snapshot(
                        db_path,
                        log_level,
                        chain.clone(),
                        *compression,
                        InclusionFilter::Cuckoo,
                        phf,
                    )?,
                    SnapshotSegment::Receipts => self.bench_receipts_snapshot(
                        db_path,
                        log_level,
                        chain.clone(),
                        *compression,
                        InclusionFilter::Cuckoo,
                        phf,
                    )?,
                }
            }
        }

        Ok(())
    }

    /// Generates successive inclusive block ranges up to the tip starting at `self.from`.
    fn block_ranges(&self, tip: BlockNumber) -> Vec<RangeInclusive<BlockNumber>> {
        let mut from = self.from;
        let mut ranges = Vec::new();

        while from <= tip {
            let end_range = std::cmp::min(from + self.block_interval - 1, tip);
            ranges.push(from..=end_range);
            from = end_range + 1;
        }

        ranges
    }

    /// Generates snapshots from `self.from` with a `self.block_interval`. Generates them in
    /// parallel if specified.
    fn generate_snapshot<DB: Database>(
        &self,
        factory: Arc<ProviderFactory<DB>>,
        segment: impl Segment + Send + Sync,
    ) -> eyre::Result<()> {
        let dir = PathBuf::default();
        let ranges = self.block_ranges(factory.last_block_number()?);

        let mut created_snapshots = vec![];

        // Filter/PHF is memory intensive, so we have to limit the parallelism.
        for block_ranges in ranges.chunks(self.parallel as usize) {
            let created_files = block_ranges
                .into_par_iter()
                .map(|block_range| {
                    let provider = factory.provider()?;

                    if !self.only_stats {
                        segment.snapshot::<DB>(&provider, &dir, block_range.clone())?;
                    }

                    let tx_range =
                        provider.transaction_range_by_block_range(block_range.clone())?;

                    Ok(segment.segment().filename(block_range, &tx_range))
                })
                .collect::<Result<Vec<_>, eyre::Report>>()?;

            created_snapshots.extend(created_files);
        }

        self.stats(created_snapshots)
    }

    /// Prints detailed statistics for each snapshot, including loading time.
    ///
    /// This function loads each snapshot from the provided paths and prints
    /// statistics about various aspects of each snapshot, such as filters size,
    /// offset index size, offset list size, and loading time.
    fn stats(&self, snapshots: Vec<impl AsRef<Path>>) -> eyre::Result<()> {
        let mut total_filters_size = 0;
        let mut total_index_size = 0;
        let mut total_duration = Duration::new(0, 0);
        let mut total_file_size = 0;

        for snap in &snapshots {
            let start_time = Instant::now();
            let jar = NippyJar::<SegmentHeader>::load(snap.as_ref())?;
            let _cursor = NippyJarCursor::new(&jar)?;
            let duration = start_time.elapsed();
            let file_size = snap.as_ref().metadata()?.len();

            total_filters_size += jar.filter_size();
            total_index_size += jar.offsets_index_size();
            total_duration += duration;
            total_file_size += file_size;

            println!("Snapshot: {:?}", snap.as_ref().file_name());
            println!("  File Size:           {:>7}", human_bytes(file_size as f64));
            println!("  Filters Size:        {:>7}", human_bytes(jar.filter_size() as f64));
            println!("  Offset Index Size:   {:>7}", human_bytes(jar.offsets_index_size() as f64));
            println!(
                "  Loading Time:        {:>7.2} ms | {:>7.2} µs",
                duration.as_millis() as f64,
                duration.as_micros() as f64
            );
        }

        let avg_duration = total_duration / snapshots.len() as u32;

        println!("Total Filters Size:     {:>7}", human_bytes(total_filters_size as f64));
        println!("Total Offset Index Size: {:>7}", human_bytes(total_index_size as f64));
        println!("Total File Size:         {:>7}", human_bytes(total_file_size as f64));
        println!(
            "Average Loading Time:    {:>7.2} ms | {:>7.2} µs",
            avg_duration.as_millis() as f64,
            avg_duration.as_micros() as f64
        );

        Ok(())
    }
}

use clap::{builder::RangedU64ValueParser, Parser};
use human_bytes::human_bytes;
use itertools::Itertools;
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use reth_db::{
    database::Database,
    mdbx::{DatabaseArguments, MaxReadTransactionDuration},
    open_db_read_only, DatabaseEnv,
};
use reth_nippy_jar::{NippyJar, NippyJarCursor};
use reth_node_core::dirs::{ChainPath, DataDirPath};
use reth_primitives::{
    static_file::{
        Compression, Filters, InclusionFilter, PerfectHashingFunction, SegmentConfig,
        SegmentHeader, SegmentRangeInclusive,
    },
    BlockNumber, ChainSpec, StaticFileSegment,
};
use reth_provider::{BlockNumReader, ProviderFactory};
use reth_static_file::{segments as static_file_segments, segments::Segment};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

mod bench;
mod headers;
mod receipts;
mod transactions;

#[derive(Parser, Debug)]
/// Arguments for the `reth db create-static-files` command.
pub struct Command {
    /// Static File segments to generate.
    segments: Vec<StaticFileSegment>,

    /// Starting block for the static file.
    #[arg(long, short, default_value = "0")]
    from: BlockNumber,

    /// Number of blocks in the static file.
    #[arg(long, short, default_value = "500000")]
    block_interval: u64,

    /// Sets the number of static files built in parallel. Note: Each parallel build is
    /// memory-intensive.
    #[arg(
        long, short,
        default_value = "1",
        value_parser = RangedU64ValueParser::<u64>::new().range(1..)
    )]
    parallel: u64,

    /// Flag to skip static file creation and print static files stats.
    #[arg(long, default_value = "false")]
    only_stats: bool,

    /// Flag to enable database-to-static file benchmarking.
    #[arg(long, default_value = "false")]
    bench: bool,

    /// Flag to skip static file creation and only run benchmarks on existing static files.
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
    /// Execute `db create-static-files` command
    pub fn execute(
        self,
        data_dir: ChainPath<DataDirPath>,
        db_args: DatabaseArguments,
        chain: Arc<ChainSpec>,
    ) -> eyre::Result<()> {
        let all_combinations = self
            .segments
            .iter()
            .cartesian_product(self.compression.iter().copied())
            .cartesian_product(if self.phf.is_empty() {
                vec![None]
            } else {
                self.phf.iter().copied().map(Some).collect::<Vec<_>>()
            });

        let db = open_db_read_only(
            data_dir.db_path().as_path(),
            db_args.with_max_read_transaction_duration(Some(MaxReadTransactionDuration::Unbounded)),
        )?;
        let provider_factory =
            Arc::new(ProviderFactory::new(db, chain, data_dir.static_files_path())?);

        {
            if !self.only_bench {
                for ((mode, compression), phf) in all_combinations.clone() {
                    let filters = if let Some(phf) = self.with_filters.then_some(phf).flatten() {
                        Filters::WithFilters(InclusionFilter::Cuckoo, phf)
                    } else {
                        Filters::WithoutFilters
                    };

                    match mode {
                        StaticFileSegment::Headers => self.generate_static_file::<DatabaseEnv>(
                            provider_factory.clone(),
                            static_file_segments::Headers,
                            SegmentConfig { filters, compression },
                        )?,
                        StaticFileSegment::Transactions => self
                            .generate_static_file::<DatabaseEnv>(
                                provider_factory.clone(),
                                static_file_segments::Transactions,
                                SegmentConfig { filters, compression },
                            )?,
                        StaticFileSegment::Receipts => self.generate_static_file::<DatabaseEnv>(
                            provider_factory.clone(),
                            static_file_segments::Receipts,
                            SegmentConfig { filters, compression },
                        )?,
                    }
                }
            }
        }

        if self.only_bench || self.bench {
            for ((mode, compression), phf) in all_combinations {
                match mode {
                    StaticFileSegment::Headers => self.bench_headers_static_file(
                        provider_factory.clone(),
                        compression,
                        InclusionFilter::Cuckoo,
                        phf,
                    )?,
                    StaticFileSegment::Transactions => self.bench_transactions_static_file(
                        provider_factory.clone(),
                        compression,
                        InclusionFilter::Cuckoo,
                        phf,
                    )?,
                    StaticFileSegment::Receipts => self.bench_receipts_static_file(
                        provider_factory.clone(),
                        compression,
                        InclusionFilter::Cuckoo,
                        phf,
                    )?,
                }
            }
        }

        Ok(())
    }

    /// Generates successive inclusive block ranges up to the tip starting at `self.from`.
    fn block_ranges(&self, tip: BlockNumber) -> Vec<SegmentRangeInclusive> {
        let mut from = self.from;
        let mut ranges = Vec::new();

        while from <= tip {
            let end_range = std::cmp::min(from + self.block_interval - 1, tip);
            ranges.push(SegmentRangeInclusive::new(from, end_range));
            from = end_range + 1;
        }

        ranges
    }

    /// Generates static files from `self.from` with a `self.block_interval`. Generates them in
    /// parallel if specified.
    fn generate_static_file<DB: Database>(
        &self,
        factory: Arc<ProviderFactory<DB>>,
        segment: impl Segment<DB>,
        config: SegmentConfig,
    ) -> eyre::Result<()> {
        let dir = PathBuf::default();
        let ranges = self.block_ranges(factory.best_block_number()?);

        let mut created_static_files = vec![];

        // Filter/PHF is memory intensive, so we have to limit the parallelism.
        for block_ranges in ranges.chunks(self.parallel as usize) {
            let created_files = block_ranges
                .into_par_iter()
                .map(|block_range| {
                    let provider = factory.provider()?;

                    if !self.only_stats {
                        segment.create_static_file_file(
                            &provider,
                            dir.as_path(),
                            config,
                            block_range.into(),
                        )?;
                    }

                    Ok(segment.segment().filename(block_range))
                })
                .collect::<Result<Vec<_>, eyre::Report>>()?;

            created_static_files.extend(created_files);
        }

        self.stats(created_static_files)
    }

    /// Prints detailed statistics for each static file, including loading time.
    ///
    /// This function loads each static file from the provided paths and prints
    /// statistics about various aspects of each static file, such as filters size,
    /// offset index size, offset list size, and loading time.
    fn stats(&self, static_files: Vec<impl AsRef<Path>>) -> eyre::Result<()> {
        let mut total_filters_size = 0;
        let mut total_index_size = 0;
        let mut total_duration = Duration::new(0, 0);
        let mut total_file_size = 0;

        for snap in &static_files {
            let start_time = Instant::now();
            let jar = NippyJar::<SegmentHeader>::load(snap.as_ref())?;
            let _cursor = NippyJarCursor::new(&jar)?;
            let duration = start_time.elapsed();
            let file_size = snap.as_ref().metadata()?.len();

            total_filters_size += jar.filter_size();
            total_index_size += jar.offsets_index_size();
            total_duration += duration;
            total_file_size += file_size;

            println!("StaticFile: {:?}", snap.as_ref().file_name());
            println!("  File Size:           {:>7}", human_bytes(file_size as f64));
            println!("  Filters Size:        {:>7}", human_bytes(jar.filter_size() as f64));
            println!("  Offset Index Size:   {:>7}", human_bytes(jar.offsets_index_size() as f64));
            println!(
                "  Loading Time:        {:>7.2} ms | {:>7.2} µs",
                duration.as_millis() as f64,
                duration.as_micros() as f64
            );
        }

        let avg_duration = total_duration / static_files.len() as u32;

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

use clap::Parser;
use itertools::Itertools;
use reth_db::{open_db_read_only, DatabaseEnv};
use reth_interfaces::db::LogLevel;
use reth_nippy_jar::NippyJar;
use reth_primitives::{
    snapshot::{Compression, InclusionFilter, PerfectHashingFunction, SegmentHeader},
    BlockNumber, ChainSpec, SnapshotSegment,
};
use reth_provider::ProviderFactory;
use std::{ops::RangeInclusive, path::Path, sync::Arc};

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
    #[arg(long, short, value_delimiter = ',', default_value = "lz4")]
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
        let all_combinations = self
            .segments
            .iter()
            .cartesian_product(self.compression.iter())
            .cartesian_product(self.phf.iter());

        {
            let db = open_db_read_only(db_path, None)?;
            let factory = ProviderFactory::new(db, chain.clone());
            let provider = factory.provider()?;

            if !self.only_bench {
                for ((mode, compression), phf) in all_combinations.clone() {
                    match mode {
                        SnapshotSegment::Headers => {
                            self.stats(self.generate_headers_snapshot::<DatabaseEnv>(
                                &provider,
                                *compression,
                                InclusionFilter::Cuckoo,
                                *phf,
                            )?)?
                        }
                        SnapshotSegment::Transactions => {
                            self.stats(self.generate_transactions_snapshot::<DatabaseEnv>(
                                &provider,
                                *compression,
                                InclusionFilter::Cuckoo,
                                *phf,
                            )?)?
                        }
                        SnapshotSegment::Receipts => {
                            self.stats(self.generate_receipts_snapshot::<DatabaseEnv>(
                                &provider,
                                *compression,
                                InclusionFilter::Cuckoo,
                                *phf,
                            )?)?
                        }
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
                        *phf,
                    )?,
                    SnapshotSegment::Transactions => self.bench_transactions_snapshot(
                        db_path,
                        log_level,
                        chain.clone(),
                        *compression,
                        InclusionFilter::Cuckoo,
                        *phf,
                    )?,
                    SnapshotSegment::Receipts => self.bench_receipts_snapshot(
                        db_path,
                        log_level,
                        chain.clone(),
                        *compression,
                        InclusionFilter::Cuckoo,
                        *phf,
                    )?,
                }
            }
        }

        Ok(())
    }

    /// Generates successive inclusive block ranges up to the tip starting at `self.from`.
    fn next_block_range(
        &self,
        from: &mut BlockNumber,
        tip: BlockNumber,
    ) -> Option<RangeInclusive<BlockNumber>> {
        if *from > tip {
            return None
        }

        let end_range = std::cmp::min(*from + self.block_interval - 1, tip);

        let range = *from..=end_range;
        *from = end_range + 1;

        return Some(range)
    }

    fn stats(&self, snapshots: Vec<impl AsRef<Path>>) -> eyre::Result<()> {
        for snap in snapshots {
            let jar = NippyJar::<SegmentHeader>::load(&snap.as_ref())?;
            println!(
                "jar: {:#?} | filters_size {} | offset_index_size {} | offsets_size {} ",
                snap.as_ref().file_name(),
                jar.offsets_index_size(),
                jar.offsets_size(),
                jar.filter_size()
            );
        }
        Ok(())
    }
}

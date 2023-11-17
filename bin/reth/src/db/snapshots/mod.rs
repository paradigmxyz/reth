use clap::Parser;
use itertools::Itertools;
use reth_db::{open_db_read_only, DatabaseEnv};
use reth_interfaces::db::LogLevel;
use reth_primitives::{
    snapshot::{Compression, InclusionFilter, PerfectHashingFunction},
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
    #[arg(long, default_value = "true")]
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
                        SnapshotSegment::Headers => self.generate_headers_snapshot::<DatabaseEnv>(
                            &provider,
                            *compression,
                            InclusionFilter::Cuckoo,
                            *phf,
                        )?,
                        SnapshotSegment::Transactions => self
                            .generate_transactions_snapshot::<DatabaseEnv>(
                                &provider,
                                *compression,
                                InclusionFilter::Cuckoo,
                                *phf,
                            )?,
                        SnapshotSegment::Receipts => self
                            .generate_receipts_snapshot::<DatabaseEnv>(
                                &provider,
                                *compression,
                                InclusionFilter::Cuckoo,
                                *phf,
                            )?,
                    }
                }
            }
        }

        if self.only_bench || self.bench {
            for ((mode, compression), phf) in all_combinations {
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

    /// Gives out the inclusive block range for the snapshot requested by the user.
    fn block_range(&self) -> RangeInclusive<BlockNumber> {
        self.from..=(self.from + self.block_interval - 1)
    }
}

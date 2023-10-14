use crate::{db::genesis_value_parser, utils::DbTool};
use clap::Parser;
use itertools::Itertools;
use reth_db::open_db_read_only;
use reth_interfaces::db::LogLevel;
use reth_nippy_jar::{
    compression::{DecoderDictionary, Decompressor},
    NippyJar,
};
use reth_primitives::{
    snapshot::{Compression, InclusionFilter, PerfectHashingFunction},
    BlockNumber, ChainSpec, SnapshotSegment,
};
use reth_provider::providers::SnapshotProvider;
use std::{path::Path, sync::Arc};

mod bench;
mod headers;

#[derive(Parser, Debug)]
/// Arguments for the `reth db snapshot` command.
pub struct Command {
    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    ///
    /// Built-in chains:
    /// - mainnet
    /// - goerli
    /// - sepolia
    /// - holesky
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        verbatim_doc_comment,
        default_value = "mainnet",
        value_parser = genesis_value_parser,
        global = true,
    )]
    chain: Arc<ChainSpec>,

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
            let tool = DbTool::new(&db, chain.clone())?;

            if !self.only_bench {
                for ((mode, compression), phf) in all_combinations.clone() {
                    match mode {
                        SnapshotSegment::Headers => self.generate_headers_snapshot(
                            &tool,
                            *compression,
                            InclusionFilter::Cuckoo,
                            *phf,
                        )?,
                        SnapshotSegment::Transactions => todo!(),
                        SnapshotSegment::Receipts => todo!(),
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
                    SnapshotSegment::Transactions => todo!(),
                    SnapshotSegment::Receipts => todo!(),
                }
            }
        }

        Ok(())
    }

    /// Returns a [`SnapshotProvider`] of the provided [`NippyJar`], alongside a list of
    /// [`DecoderDictionary`] and [`Decompressor`] if necessary.
    fn prepare_jar_provider<'a>(
        &self,
        jar: &'a mut NippyJar,
        dictionaries: &'a mut Option<Vec<DecoderDictionary<'_>>>,
    ) -> eyre::Result<(SnapshotProvider<'a>, Vec<Decompressor<'a>>)> {
        let mut decompressors: Vec<Decompressor<'_>> = vec![];
        if let Some(reth_nippy_jar::compression::Compressors::Zstd(zstd)) = jar.compressor_mut() {
            if zstd.use_dict {
                *dictionaries = zstd.generate_decompress_dictionaries();
                decompressors = zstd.generate_decompressors(dictionaries.as_ref().expect("qed"))?;
            }
        }

        Ok((SnapshotProvider { jar: &*jar, jar_start_block: self.from }, decompressors))
    }
}

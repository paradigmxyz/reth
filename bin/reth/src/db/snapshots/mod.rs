use crate::utils::DbTool;
use clap::{clap_derive::ValueEnum, Parser};
use eyre::WrapErr;
use itertools::Itertools;
use reth_db::{database::Database, open_db_read_only, table::Table, tables, DatabaseEnvRO};
use reth_interfaces::db::LogLevel;
use reth_nippy_jar::{
    compression::{DecoderDictionary, Decompressor},
    NippyJar,
};
use reth_primitives::ChainSpec;
use reth_provider::providers::SnapshotProvider;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

mod bench;
mod headers;

pub(crate) type Rows = Vec<Vec<Vec<u8>>>;
pub(crate) type JarConfig = (Snapshots, Compression, PerfectHashingFunction);

#[derive(Parser, Debug)]
/// Arguments for the `reth db snapshot` command.
pub struct Command {
    /// Snapshot categories to generate.
    modes: Vec<Snapshots>,

    /// Starting block for the snapshot.
    #[arg(long, short, default_value = "0")]
    from: usize,

    /// Number of blocks in the snapshot.
    #[arg(long, short, default_value = "500000")]
    block_interval: usize,

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
    #[arg(long, value_delimiter = ',', default_value_if("with_filters", "true", "mphf"))]
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
            .modes
            .iter()
            .cartesian_product(self.compression.iter())
            .cartesian_product(self.phf.iter());

        {
            let db = open_db_read_only(db_path, None)?;
            let tool = DbTool::new(&db, chain.clone())?;

            if !self.only_bench {
                for ((mode, compression), phf) in all_combinations.clone() {
                    match mode {
                        Snapshots::Headers => {
                            self.generate_headers_snapshot(&tool, *compression, *phf)?
                        }
                        Snapshots::Transactions => todo!(),
                        Snapshots::Receipts => todo!(),
                    }
                }
            }
        }

        if self.only_bench || self.bench {
            for ((mode, compression), phf) in all_combinations {
                match mode {
                    Snapshots::Headers => self.bench_headers_snapshot(
                        db_path,
                        log_level,
                        chain.clone(),
                        *compression,
                        *phf,
                    )?,
                    Snapshots::Transactions => todo!(),
                    Snapshots::Receipts => todo!(),
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

        Ok((SnapshotProvider { jar: &*jar, jar_start_block: self.from as u64 }, decompressors))
    }

    /// Returns a [`NippyJar`] according to the desired configuration.
    fn prepare_jar<F: Fn() -> eyre::Result<Rows>>(
        &self,
        num_columns: usize,
        jar_config: JarConfig,
        tool: &DbTool<'_, DatabaseEnvRO>,
        prepare_compression: F,
    ) -> eyre::Result<NippyJar> {
        let (mode, compression, phf) = jar_config;
        let snap_file = self.get_file_path(jar_config);
        let table_name = match mode {
            Snapshots::Headers => tables::Headers::NAME,
            Snapshots::Transactions | Snapshots::Receipts => tables::Transactions::NAME,
        };

        let total_rows = tool.db.view(|tx| {
            let table_db = tx.inner.open_db(Some(table_name)).wrap_err("Could not open db.")?;
            let stats = tx
                .inner
                .db_stat(&table_db)
                .wrap_err(format!("Could not find table: {}", table_name))?;

            Ok::<usize, eyre::Error>((stats.entries() - self.from).min(self.block_interval))
        })??;

        assert!(
            total_rows >= self.block_interval,
            "Not enough rows on database {} < {}.",
            total_rows,
            self.block_interval
        );

        let mut nippy_jar = NippyJar::new_without_header(num_columns, snap_file.as_path());
        nippy_jar = match compression {
            Compression::Lz4 => nippy_jar.with_lz4(),
            Compression::Zstd => nippy_jar.with_zstd(false, 0),
            Compression::ZstdWithDictionary => {
                let dataset = prepare_compression()?;

                nippy_jar = nippy_jar.with_zstd(true, 5_000_000);
                nippy_jar.prepare_compression(dataset)?;
                nippy_jar
            }
            Compression::Uncompressed => nippy_jar,
        };

        if self.with_filters {
            nippy_jar = nippy_jar.with_cuckoo_filter(self.block_interval);
            nippy_jar = match phf {
                PerfectHashingFunction::Mphf => nippy_jar.with_mphf(),
                PerfectHashingFunction::GoMphf => nippy_jar.with_gomphf(),
            };
        }

        Ok(nippy_jar)
    }

    /// Generates a filename according to the desired configuration.
    fn get_file_path(&self, jar_config: JarConfig) -> PathBuf {
        let (mode, compression, phf) = jar_config;
        format!(
            "snapshot_{mode:?}_{}_{}_{compression:?}_{phf:?}",
            self.from,
            self.from + self.block_interval
        )
        .into()
    }
}

#[derive(Debug, Copy, Clone, ValueEnum)]
pub(crate) enum Snapshots {
    Headers,
    Transactions,
    Receipts,
}

#[derive(Debug, Copy, Clone, ValueEnum, Default)]
pub(crate) enum Compression {
    Lz4,
    Zstd,
    ZstdWithDictionary,
    #[default]
    Uncompressed,
}

#[derive(Debug, Copy, Clone, ValueEnum)]
pub(crate) enum PerfectHashingFunction {
    Mphf,
    GoMphf,
}

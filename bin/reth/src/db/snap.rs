use crate::utils::DbTool;
use clap::{clap_derive::ValueEnum, Parser};
use eyre::WrapErr;
use rand::{seq::SliceRandom, Rng};
use reth_db::{
    cursor::DbCursorRO,
    database::Database,
    open_db_read_only,
    snapshot::create_snapshot_T1_T2,
    table::{Decompress, Table},
    tables,
    transaction::DbTx,
    DatabaseEnvRO,
};
use reth_interfaces::db::LogLevel;
use reth_nippy_jar::{
    compression::{DecoderDictionary, Decompressor},
    NippyJar,
};
use reth_primitives::{BlockNumber, ChainSpec, Header};
use reth_provider::{
    providers::SnapshotProvider, DatabaseProviderRO, HeaderProvider, ProviderError, ProviderFactory,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};
use tables::*;

#[derive(Parser, Debug)]
/// The arguments for the `reth db snapshot` command
pub struct Command {
    /// Which snapshot categories to create
    modes: Vec<Snapshots>,
    /// From which block to snapshot from
    #[arg(long, short, default_value = "0")]
    from: usize,
    /// How many blocks to snapshot
    #[arg(long, short, default_value = "500000")]
    block_interval: usize,
    /// Print out a quick benchmark between the database and the created snapshots
    #[arg(long, default_value = "false")]
    bench: bool,
    /// Does not create a snapshot, and prints out a quick benchmark between the database and
    /// previous snapshots
    #[arg(long, default_value = "false")]
    only_bench: bool,
    /// Compression schemes to use
    #[arg(long, short, value_delimiter = ',', default_value = "lz4")]
    compression: Vec<Compression>,
    /// Whether to use inclusion list filters and PHF
    #[arg(long, default_value = "true")]
    with_filters: bool,
    /// Which perfect hashing function to use
    #[arg(long, value_delimiter = ',', default_value = "mphf")]
    phf: Vec<PHF>,
}

impl Command {
    /// Execute `db list` command
    pub fn execute(
        self,
        db_path: &Path,
        log_level: Option<LogLevel>,
        chain: Arc<ChainSpec>,
    ) -> eyre::Result<()> {
        {
            let db = open_db_read_only(db_path, None)?;
            let tool = DbTool::new(&db, chain.clone())?;

            if !self.only_bench {
                for mode in &self.modes {
                    for compression in &self.compression {
                        for phf in &self.phf {
                            match mode {
                                Snapshots::Headers => {
                                    self.generate_headers_snapshot(&tool, compression, phf)?
                                }
                                Snapshots::Transactions => todo!(),
                                Snapshots::Receipts => todo!(),
                            }
                        }
                    }
                }
            }
        }

        if self.only_bench || self.bench {
            for mode in &self.modes {
                for compression in &self.compression {
                    for phf in &self.phf {
                        match mode {
                            Snapshots::Headers => self.bench_headers_snapshot(
                                db_path,
                                log_level,
                                chain.clone(),
                                compression,
                                phf,
                            )?,
                            Snapshots::Transactions => todo!(),
                            Snapshots::Receipts => todo!(),
                        }
                    }
                }
            }
        }

        Ok(())
    }
    fn generate_headers_snapshot(
        &self,
        tool: &DbTool<'_, DatabaseEnvRO>,
        compression: &Compression,
        phf: &PHF,
    ) -> eyre::Result<()> {
        let mut jar = self.prepare_jar(2, tool, Snapshots::Headers, compression, phf, || {
            let dataset = tool.db.view(|tx| {
                let mut cursor = tx.cursor_read::<reth_db::RawTable<reth_db::Headers>>().unwrap();
                let v1 = cursor
                    .walk_back(None)
                    .unwrap()
                    .take(self.block_interval.min(1000))
                    .map(|row| row.map(|(_key, value)| value.into_value()).expect(""))
                    .collect::<Vec<_>>();
                let mut cursor = tx.cursor_read::<reth_db::RawTable<reth_db::HeaderTD>>().unwrap();
                let v2 = cursor
                    .walk_back(None)
                    .unwrap()
                    .take(self.block_interval.min(1000))
                    .map(|row| row.map(|(_key, value)| value.into_value()).expect(""))
                    .collect::<Vec<_>>();
                vec![v1, v2]
            })?;
            Ok(Some(dataset))
        })?;

        tool.db.view(|tx| {
            // Hacky type inference. TODO fix
            let mut none_vec = Some(vec![vec![vec![0u8]].into_iter()]);
            let _ = none_vec.take();

            // Generate list of hashes for filters & PHF
            let mut cursor = tx.cursor_read::<RawTable<CanonicalHeaders>>().unwrap();
            let hashes = cursor
                .walk(Some(RawKey::from(self.from as u64)))
                .unwrap()
                .take(self.block_interval)
                .map(|row| row.map(|(_key, value)| value.into_value()).map_err(|e| e.into()));

            create_snapshot_T1_T2::<Headers, HeaderTD, BlockNumber>(
                tx,
                self.from as u64..=(self.from as u64 + self.block_interval as u64),
                vec![],
                // We already prepared the dictionary beforehand
                none_vec,
                Some(hashes),
                self.block_interval,
                &mut jar,
            )
        })??;

        Ok(())
    }

    fn bench_headers_snapshot(
        &self,
        db_path: &Path,
        log_level: Option<LogLevel>,
        chain: Arc<ChainSpec>,
        compression: &Compression,
        phf: &PHF,
    ) -> eyre::Result<()> {
        let mut jar = NippyJar::load_without_header(&self.get_file_path(
            Snapshots::Headers,
            compression,
            phf,
        ))?;
        let mut row_indexes = (self.from..(self.from + self.block_interval)).collect::<Vec<_>>();
        let mut rng = rand::thread_rng();
        let mut dictionaries = None;
        let (provider, decompressors) = self.prepare_jar_provider(&mut jar, &mut dictionaries)?;
        let mut cursor = if !decompressors.is_empty() {
            provider.cursor_with_decompressors(decompressors)
        } else {
            provider.cursor()
        };
        let mode = Snapshots::Headers;

        for bench_kind in [BenchKind::Walk, BenchKind::RandomWalk] {
            let db = open_db_read_only(db_path, log_level)?;
            let tool = DbTool::new(&db, chain.clone())?;

            bench(
                mode,
                bench_kind,
                compression,
                phf,
                &tool,
                || {
                    for num in row_indexes.iter() {
                        Header::decompress(
                            cursor
                                .row_by_number_with_cols::<0b01, 2>(num - self.from)
                                .unwrap()
                                .ok_or(ProviderError::HeaderNotFound((*num as u64).into()))
                                .unwrap()[0],
                        )
                        .unwrap();
                        // TODO: replace with below when provider re-uses cursor
                        // let h = provider.header_by_number(num as u64)?.unwrap();
                    }
                },
                |provider| {
                    for num in row_indexes.iter() {
                        provider.header_by_number(*num as u64).unwrap().unwrap();
                    }
                },
            );

            // For random walk
            row_indexes.shuffle(&mut rng);
        }

        let db = open_db_read_only(db_path, log_level)?;
        let tool = DbTool::new(&db, chain.clone())?;
        let num = row_indexes[rng.gen_range(0..row_indexes.len())];
        bench(
            mode,
            BenchKind::OneRow,
            compression,
            phf,
            &tool,
            || {
                Header::decompress(
                    cursor
                        .row_by_number_with_cols::<0b01, 2>((num - self.from) as usize)
                        .unwrap()
                        .ok_or(ProviderError::HeaderNotFound((num as u64).into()))
                        .unwrap()[0],
                )
                .unwrap();
            },
            |provider| {
                provider.header_by_number(num as u64).unwrap().unwrap();
            },
        );

        Ok(())
    }

    fn prepare_jar_provider<'a>(
        &self,
        jar: &'a mut NippyJar,
        dictionaries: &'a mut Option<Vec<DecoderDictionary<'_>>>,
    ) -> eyre::Result<(SnapshotProvider<'a>, Vec<Decompressor<'a>>)> {
        let mut decompressors: Vec<Decompressor<'_>> = vec![];
        if let Some(reth_nippy_jar::compression::Compressors::Zstd(zstd)) = jar.compressor_mut() {
            if zstd.use_dict {
                *dictionaries = zstd.generate_decompress_dictionaries();
                decompressors =
                    zstd.generate_decompressors(dictionaries.as_ref().unwrap()).unwrap();
            }
        }

        Ok((SnapshotProvider { jar: &*jar, jar_start_block: self.from as u64 }, decompressors))
    }

    fn prepare_jar<F: Fn() -> eyre::Result<Option<Vec<Vec<Vec<u8>>>>>>(
        &self,
        num_columns: usize,
        tool: &DbTool<'_, DatabaseEnvRO>,
        mode: Snapshots,
        compression: &Compression,
        phf: &PHF,
        prepare_compression: F,
    ) -> eyre::Result<NippyJar> {
        let snap_file = self.get_file_path(mode, compression, phf);
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
                assert!(dataset.is_some(), "Expected a dataset for the dictionary");

                nippy_jar = nippy_jar.with_zstd(true, 5_000_000);
                nippy_jar.prepare_compression(dataset.expect("qed"))?;
                nippy_jar
            }
            Compression::Uncompressed => nippy_jar,
        };

        if self.with_filters {
            nippy_jar = nippy_jar.with_cuckoo_filter(self.block_interval);
        }

        nippy_jar = match phf {
            PHF::Mphf => nippy_jar.with_mphf(),
            PHF::GoMphf => nippy_jar.with_gomphf(),
        };

        Ok(nippy_jar)
    }

    fn get_file_path(&self, mode: Snapshots, compression: &Compression, phf: &PHF) -> PathBuf {
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
pub(crate) enum PHF {
    Mphf,
    GoMphf,
}

#[derive(Debug)]
enum BenchKind {
    Walk,
    RandomWalk,
    OneRow,
}

fn bench<F1, F2>(
    mode: Snapshots,
    bench_kind: BenchKind,
    compression: &Compression,
    phf: &PHF,
    tool: &DbTool<'_, DatabaseEnvRO>,
    mut snapshot_method: F1,
    database_method: F2,
) where
    F1: FnMut(),
    F2: Fn(DatabaseProviderRO<'_, DatabaseEnvRO>),
{
    println!();
    println!("############");
    println!("## [{mode:?}] [{compression:?}] [{phf:?}] [{bench_kind:?}]");
    {
        let start = Instant::now();
        snapshot_method();
        let end = start.elapsed().as_micros();
        println!("# snapshot {bench_kind:?} | {end} μs");
    }
    {
        let factory = ProviderFactory::new(tool.db, tool.chain.clone());
        let provider = factory.provider().unwrap();
        let start = Instant::now();
        database_method(provider);
        let end = start.elapsed().as_micros();
        println!("# database {bench_kind:?} | {end} μs");
    }
}

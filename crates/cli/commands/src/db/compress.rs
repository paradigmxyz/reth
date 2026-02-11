use clap::{Parser, ValueEnum};
use human_bytes::human_bytes;
use reth_db_api::{
    cursor::DbCursorRO, table::Table, transaction::DbTx, RawKey, RawTable, RawValue, TableViewer,
    Tables,
};
use reth_db_common::DbTool;
use reth_libmdbx::{DatabaseFlags, Environment, Geometry, WriteFlags};
use reth_node_builder::NodeTypesWithDB;
use reth_provider::{providers::ProviderNodeTypes, DBProvider};
use std::{path::PathBuf, time::Instant};
use tracing::{info, warn};

const PROGRESS_LOG_INTERVAL: usize = 100_000;

/// Batch size for MDBX write transactions to avoid unbounded growth.
const COMMIT_BATCH_SIZE: usize = 50_000;

/// Default number of samples for zstd dictionary training.
const DEFAULT_MAX_SAMPLES: usize = 100_000;

/// Default zstd dictionary size (112 KiB, matching reth's existing dictionaries).
const DEFAULT_DICT_SIZE: usize = 112_640;

/// Compression algorithm to use.
#[derive(Debug, Clone, Copy, Default, ValueEnum)]
pub enum CompressionType {
    /// LZ4 block compression (fast, no dictionary).
    #[default]
    Lz4,
    /// Zstd compression with a trained dictionary (better ratio, two-pass).
    ZstdDict,
}

/// Compresses all values from a table and writes them to a new MDBX database.
///
/// Creates a fresh MDBX environment at `--out`, with a single table named after the source.
/// Each row is written as `(original_key, compressed_value)`, preserving key ordering.
/// This allows apples-to-apples comparison of on-disk size including btree/page overhead.
#[derive(Parser, Debug)]
pub struct Command {
    /// The source table to compress.
    table: Tables,

    /// Path for the output MDBX database directory.
    #[arg(long)]
    out: PathBuf,

    /// Compression algorithm.
    #[arg(long, value_enum, default_value_t = CompressionType::Lz4)]
    compression: CompressionType,

    /// Path to write the trained zstd dictionary (only used with `--compression zstd-dict`).
    #[arg(long)]
    dict_out: Option<PathBuf>,

    /// Maximum number of value samples for dictionary training (only with `zstd-dict`).
    #[arg(long, default_value_t = DEFAULT_MAX_SAMPLES)]
    max_samples: usize,

    /// Maximum dictionary size in bytes (only with `zstd-dict`).
    #[arg(long, default_value_t = DEFAULT_DICT_SIZE)]
    dict_size: usize,

    /// Zstd compression level. 0 = default (3). Higher = better ratio, slower.
    #[arg(long, default_value_t = 3)]
    zstd_level: i32,
}

impl Command {
    /// Execute `db compress` command
    pub fn execute<N: ProviderNodeTypes>(self, tool: &DbTool<N>) -> eyre::Result<()> {
        warn!("This command should be run without the node running!");
        self.table.view(&CompressViewer {
            tool,
            out: self.out,
            compression: self.compression,
            dict_out: self.dict_out,
            max_samples: self.max_samples,
            dict_size: self.dict_size,
            zstd_level: self.zstd_level,
        })
    }
}

struct CompressViewer<'a, N: NodeTypesWithDB> {
    tool: &'a DbTool<N>,
    out: PathBuf,
    compression: CompressionType,
    dict_out: Option<PathBuf>,
    max_samples: usize,
    dict_size: usize,
    zstd_level: i32,
}

/// Opens a fresh output MDBX environment at `path` and creates a table named `table_name`.
fn open_output_env(
    path: &PathBuf,
    table_name: &str,
) -> eyre::Result<(Environment, reth_libmdbx::Database)> {
    std::fs::create_dir_all(path)
        .map_err(|e| eyre::eyre!("Failed to create output dir {path:?}: {e}"))?;

    let env = {
        let mut builder = Environment::builder();
        builder.set_max_dbs(1);
        builder.write_map();
        builder.set_geometry(Geometry {
            size: Some(0..(1024 * 1024 * 1024 * 1024)), // up to 1 TiB
            ..Default::default()
        });
        builder
            .open(path)
            .map_err(|e| eyre::eyre!("Failed to open output MDBX at {path:?}: {e}"))?
    };

    let db = {
        let tx = env.begin_rw_txn().map_err(|e| eyre::eyre!("Failed to begin write txn: {e}"))?;
        let db = tx
            .create_db(Some(table_name), DatabaseFlags::empty())
            .map_err(|e| eyre::eyre!("Failed to create table `{table_name}`: {e}"))?;
        tx.commit().map_err(|e| eyre::eyre!("Failed to commit table creation: {e}"))?;
        db
    };

    Ok((env, db))
}

/// Trait abstracting over compression algorithms for the write loop.
trait Compressor {
    fn compress(&mut self, data: &[u8]) -> eyre::Result<Vec<u8>>;
    fn decompress(&self, compressed: &[u8], original_len: usize) -> eyre::Result<Vec<u8>>;
    fn name(&self) -> &'static str;
}

struct Lz4Compressor;

impl Compressor for Lz4Compressor {
    fn compress(&mut self, data: &[u8]) -> eyre::Result<Vec<u8>> {
        lz4::block::compress(data, None, true)
            .map_err(|e| eyre::eyre!("LZ4 compression failed: {e}"))
    }

    fn decompress(&self, compressed: &[u8], _original_len: usize) -> eyre::Result<Vec<u8>> {
        // Size prefix is embedded (prepend_size=true during compress)
        lz4::block::decompress(compressed, None)
            .map_err(|e| eyre::eyre!("LZ4 decompression failed: {e}"))
    }

    fn name(&self) -> &'static str {
        "LZ4"
    }
}

struct ZstdDictCompressor {
    compressor: zstd::bulk::Compressor<'static>,
    dictionary: Vec<u8>,
}

impl ZstdDictCompressor {
    fn new(dictionary: Vec<u8>, level: i32) -> eyre::Result<Self> {
        let compressor = zstd::bulk::Compressor::with_dictionary(level, &dictionary)
            .map_err(|e| eyre::eyre!("Failed to create zstd compressor: {e}"))?;
        Ok(Self { compressor, dictionary })
    }
}

impl Compressor for ZstdDictCompressor {
    fn compress(&mut self, data: &[u8]) -> eyre::Result<Vec<u8>> {
        self.compressor.compress(data).map_err(|e| eyre::eyre!("Zstd compression failed: {e}"))
    }

    fn decompress(&self, compressed: &[u8], original_len: usize) -> eyre::Result<Vec<u8>> {
        let mut decompressor = zstd::bulk::Decompressor::with_dictionary(&self.dictionary)
            .map_err(|e| eyre::eyre!("Failed to create zstd decompressor: {e}"))?;
        decompressor
            .decompress(compressed, original_len)
            .map_err(|e| eyre::eyre!("Zstd decompression failed: {e}"))
    }

    fn name(&self) -> &'static str {
        "Zstd+dict"
    }
}

/// Stats returned from the compression pass.
type CompressResult = (usize, u64, u64, Vec<(Vec<u8>, Vec<u8>)>);

/// Writes compressed rows to the output MDBX, returning (row_count, original_bytes,
/// compressed_bytes, verify_samples).
fn compress_to_mdbx<T: Table>(
    tx: &impl DbTx,
    out_env: &Environment,
    out_dbi: &reth_libmdbx::Database,
    compressor: &mut dyn Compressor,
) -> eyre::Result<CompressResult> {
    let mut cursor = tx.cursor_read::<RawTable<T>>()?;
    let mut row_count = 0usize;
    let mut original_bytes = 0u64;
    let mut compressed_bytes = 0u64;
    let mut verify_samples: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();

    let mut out_tx =
        out_env.begin_rw_txn().map_err(|e| eyre::eyre!("Failed to begin write txn: {e}"))?;

    for entry in cursor.walk(None)? {
        let (k, v): (RawKey<T::Key>, RawValue<T::Value>) = entry?;

        let key_bytes = k.raw_key();
        let value_bytes = v.raw_value();
        original_bytes += value_bytes.len() as u64;

        let compressed = compressor.compress(value_bytes)?;
        compressed_bytes += compressed.len() as u64;

        if verify_samples.len() < 10 {
            verify_samples.push((value_bytes.to_vec(), compressed.clone()));
        }

        out_tx
            .put(out_dbi.dbi(), key_bytes, &compressed, WriteFlags::APPEND)
            .map_err(|e| eyre::eyre!("MDBX put failed at row {row_count}: {e}"))?;

        row_count += 1;
        if row_count.is_multiple_of(COMMIT_BATCH_SIZE) {
            out_tx
                .commit()
                .map_err(|e| eyre::eyre!("Failed to commit batch at row {row_count}: {e}"))?;
            out_tx = out_env
                .begin_rw_txn()
                .map_err(|e| eyre::eyre!("Failed to begin write txn: {e}"))?;

            if row_count.is_multiple_of(PROGRESS_LOG_INTERVAL) {
                info!("Compressed {row_count} rows");
            }
        }
    }

    out_tx.commit().map_err(|e| eyre::eyre!("Failed to final commit: {e}"))?;

    Ok((row_count, original_bytes, compressed_bytes, verify_samples))
}

impl<N: ProviderNodeTypes> TableViewer<()> for CompressViewer<'_, N> {
    type Error = eyre::Report;

    fn view<T: Table>(&self) -> Result<(), Self::Error> {
        let provider =
            self.tool.provider_factory.provider()?.disable_long_read_transaction_safety();
        let tx = provider.tx_ref();

        let (out_env, out_dbi) = open_output_env(&self.out, T::NAME)?;

        let mut compressor: Box<dyn Compressor> = match self.compression {
            CompressionType::Lz4 => {
                info!("Compressing table `{}` with LZ4 -> {:?}", T::NAME, self.out);
                Box::new(Lz4Compressor)
            }
            CompressionType::ZstdDict => {
                // Pass 1: sample values for dictionary training
                info!(
                    "Pass 1: sampling up to {} values from `{}` for zstd dictionary training",
                    self.max_samples,
                    T::NAME
                );
                let sample_start = Instant::now();

                let mut cursor = tx.cursor_read::<RawTable<T>>()?;
                let mut samples: Vec<Vec<u8>> = Vec::with_capacity(self.max_samples);
                let mut total_scanned = 0usize;

                for entry in cursor.walk(None)? {
                    let (_k, v): (RawKey<T::Key>, RawValue<T::Value>) = entry?;
                    total_scanned += 1;

                    if samples.len() < self.max_samples {
                        samples.push(v.raw_value().to_vec());
                    }

                    if total_scanned.is_multiple_of(PROGRESS_LOG_INTERVAL) {
                        info!("Scanned {total_scanned} rows, collected {} samples", samples.len());
                    }
                }

                info!(
                    "Scanned {total_scanned} rows, collected {} samples in {:?}",
                    samples.len(),
                    sample_start.elapsed()
                );

                if samples.is_empty() {
                    eyre::bail!("Table `{}` is empty, nothing to compress", T::NAME);
                }

                // Train dictionary using zstd's from_continuous API
                info!(
                    "Training zstd dictionary (target {} bytes) from {} samples",
                    self.dict_size,
                    samples.len()
                );
                let train_start = Instant::now();

                let mut sizes = Vec::with_capacity(samples.len());
                let data: Vec<u8> = samples
                    .iter()
                    .flat_map(|s| {
                        sizes.push(s.len());
                        s.iter().copied()
                    })
                    .collect();

                let dictionary = zstd::dict::from_continuous(&data, &sizes, self.dict_size)
                    .map_err(|e| eyre::eyre!("Failed to train zstd dictionary: {e}"))?;

                info!(
                    "Dictionary trained in {:?} ({} bytes)",
                    train_start.elapsed(),
                    dictionary.len()
                );

                // Optionally write dictionary to file
                if let Some(dict_path) = &self.dict_out {
                    reth_fs_util::write(dict_path, &dictionary)?;
                    info!("Dictionary written to {dict_path:?}");
                }

                info!("Pass 2: compressing table `{}` with Zstd+dict -> {:?}", T::NAME, self.out);
                Box::new(ZstdDictCompressor::new(dictionary, self.zstd_level)?)
            }
        };

        let start = Instant::now();

        let (row_count, original_bytes, compressed_bytes, verify_samples) =
            compress_to_mdbx::<T>(tx, &out_env, &out_dbi, compressor.as_mut())?;

        let elapsed = start.elapsed();
        let ratio =
            if original_bytes > 0 { compressed_bytes as f64 / original_bytes as f64 } else { 0.0 };

        info!(
            "Done ({}): {} rows, {} -> {} ({:.1}% of original, ratio {:.3}), elapsed {:?}",
            compressor.name(),
            row_count,
            human_bytes(original_bytes as f64),
            human_bytes(compressed_bytes as f64),
            ratio * 100.0,
            ratio,
            elapsed,
        );

        if ratio > 1.0 {
            warn!(
                "Compressed output is larger than original — \
                 data may already be in a compact/compressed format"
            );
        }

        // Round-trip verify
        info!("Verifying {} samples round-trip...", verify_samples.len());
        for (i, (original, compressed)) in verify_samples.iter().enumerate() {
            let decompressed = compressor.decompress(compressed, original.len())?;
            eyre::ensure!(
                decompressed == *original,
                "Round-trip verification failed on sample {i}"
            );
        }
        info!("Verification passed.");

        Ok(())
    }
}

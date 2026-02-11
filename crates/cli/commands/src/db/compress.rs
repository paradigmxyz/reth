use clap::Parser;
use human_bytes::human_bytes;
use lz4::block;
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

/// LZ4-compresses all values from a table and writes them to a new MDBX database.
///
/// Creates a fresh MDBX environment at `--out`, with a single table named after the source.
/// Each row is written as `(original_key, lz4_compressed_value)`, preserving key ordering.
/// This allows apples-to-apples comparison of on-disk size including btree/page overhead.
#[derive(Parser, Debug)]
pub struct Command {
    /// The source table to compress.
    table: Tables,

    /// Path for the output MDBX database directory.
    #[arg(long)]
    out: PathBuf,
}

impl Command {
    /// Execute `db compress` command
    pub fn execute<N: ProviderNodeTypes>(self, tool: &DbTool<N>) -> eyre::Result<()> {
        warn!("This command should be run without the node running!");
        self.table.view(&CompressViewer { tool, out: self.out })
    }
}

struct CompressViewer<'a, N: NodeTypesWithDB> {
    tool: &'a DbTool<N>,
    out: PathBuf,
}

impl<N: ProviderNodeTypes> TableViewer<()> for CompressViewer<'_, N> {
    type Error = eyre::Report;

    fn view<T: Table>(&self) -> Result<(), Self::Error> {
        let provider =
            self.tool.provider_factory.provider()?.disable_long_read_transaction_safety();
        let tx = provider.tx_ref();

        // Create output MDBX environment
        std::fs::create_dir_all(&self.out)
            .map_err(|e| eyre::eyre!("Failed to create output dir {:?}: {e}", self.out))?;

        let out_env = {
            let mut builder = Environment::builder();
            builder.set_max_dbs(1);
            builder.write_map();
            builder.set_geometry(Geometry {
                size: Some(0..(1024 * 1024 * 1024 * 1024)), // up to 1 TiB
                ..Default::default()
            });
            builder
                .open(&self.out)
                .map_err(|e| eyre::eyre!("Failed to open output MDBX at {:?}: {e}", self.out))?
        };

        // Create the output table with the same name as the source
        let out_dbi = {
            let out_tx = out_env
                .begin_rw_txn()
                .map_err(|e| eyre::eyre!("Failed to begin write txn: {e}"))?;
            let db = out_tx
                .create_db(Some(T::NAME), DatabaseFlags::empty())
                .map_err(|e| eyre::eyre!("Failed to create table `{}`: {e}", T::NAME))?;
            out_tx.commit().map_err(|e| eyre::eyre!("Failed to commit table creation: {e}"))?;
            db
        };

        info!("Compressing table `{}` with LZ4 -> {:?}", T::NAME, self.out);
        let start = Instant::now();

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

            // LZ4 block compress with size prefix for self-describing decompression
            let compressed = block::compress(value_bytes, None, true)
                .map_err(|e| eyre::eyre!("LZ4 compression failed at row {row_count}: {e}"))?;
            compressed_bytes += compressed.len() as u64;

            // Keep a few samples for round-trip verification
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

        // Final commit
        out_tx.commit().map_err(|e| eyre::eyre!("Failed to final commit: {e}"))?;

        let elapsed = start.elapsed();
        let ratio =
            if original_bytes > 0 { compressed_bytes as f64 / original_bytes as f64 } else { 0.0 };

        info!(
            "Done: {} rows, {} -> {} ({:.1}% of original, ratio {:.3}), elapsed {:?}",
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

        // Round-trip verify samples
        info!("Verifying {} samples round-trip...", verify_samples.len());
        for (i, (original, compressed)) in verify_samples.iter().enumerate() {
            let decompressed = block::decompress(compressed, None)
                .map_err(|e| eyre::eyre!("Verification decompress failed on sample {i}: {e}"))?;
            eyre::ensure!(
                decompressed == *original,
                "Round-trip verification failed on sample {i}"
            );
        }
        info!("Verification passed.");

        Ok(())
    }
}

use clap::Parser;
use human_bytes::human_bytes;
use lz4::block;
use reth_db_api::{
    cursor::DbCursorRO, table::Table, transaction::DbTx, RawKey, RawTable, RawValue, TableViewer,
    Tables,
};
use reth_db_common::DbTool;
use reth_node_builder::NodeTypesWithDB;
use reth_provider::{providers::ProviderNodeTypes, DBProvider};
use std::{io::Write, path::PathBuf, time::Instant};
use tracing::{info, warn};

const PROGRESS_LOG_INTERVAL: usize = 100_000;

/// LZ4-compresses all values from a table and writes them to a file.
///
/// Iterates every row in the source table, compresses each value with LZ4 block mode,
/// and writes `[key_len: u32][key][original_len: u32][compressed_len: u32][compressed]` records
/// to the output file. Reports compression ratio and throughput at the end.
#[derive(Parser, Debug)]
pub struct Command {
    /// The source table to compress.
    table: Tables,

    /// Path to write the compressed table data.
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

        info!("Compressing table `{}` with LZ4 block mode", T::NAME);
        let start = Instant::now();

        let mut out_file = std::io::BufWriter::new(
            std::fs::File::create(&self.out)
                .map_err(|e| eyre::eyre!("Failed to create output file {:?}: {e}", self.out))?,
        );
        let mut cursor = tx.cursor_read::<RawTable<T>>()?;
        let mut row_count = 0usize;
        let mut original_bytes = 0u64;
        let mut compressed_bytes = 0u64;
        let mut verify_samples: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();

        for entry in cursor.walk(None)? {
            let (k, v): (RawKey<T::Key>, RawValue<T::Value>) = entry?;

            let key_bytes = k.raw_key();
            let value_bytes = v.raw_value();
            let value_len = value_bytes.len();
            original_bytes += value_len as u64;

            // LZ4 block compress without size prefix (we store original length ourselves)
            let compressed = block::compress(value_bytes, None, false)
                .map_err(|e| eyre::eyre!("LZ4 compression failed at row {row_count}: {e}"))?;
            compressed_bytes += compressed.len() as u64;

            // Keep a few samples for round-trip verification
            if verify_samples.len() < 10 {
                verify_samples.push((value_bytes.to_vec(), compressed.clone()));
            }

            // Record: [key_len: u32][key][original_len: u32][compressed_len: u32][compressed]
            let key_len: u32 = key_bytes.len().try_into().map_err(|_| {
                eyre::eyre!("Key too large ({} bytes) at row {row_count}", key_bytes.len())
            })?;
            let comp_len: u32 = compressed.len().try_into().map_err(|_| {
                eyre::eyre!(
                    "Compressed value too large ({} bytes) at row {row_count}",
                    compressed.len()
                )
            })?;

            out_file.write_all(&key_len.to_le_bytes())?;
            out_file.write_all(key_bytes)?;
            out_file.write_all(&(value_len as u32).to_le_bytes())?;
            out_file.write_all(&comp_len.to_le_bytes())?;
            out_file.write_all(&compressed)?;

            row_count += 1;
            if row_count.is_multiple_of(PROGRESS_LOG_INTERVAL) {
                info!("Compressed {row_count} rows");
            }
        }

        out_file.flush()?;

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
            let decompressed = block::decompress(compressed, Some(original.len() as i32))
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

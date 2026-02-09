use alloy_primitives::keccak256;
use clap::Parser;
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use reth_codecs::Compact;
use reth_db::{static_file::iter_static_files, DatabaseEnv};
use reth_db_api::{models::StorageBeforeTx, table::Decompress};
use reth_db_common::DbTool;
use reth_nippy_jar::{NippyJar, NippyJarWriter, CHANGESET_OFFSETS_FILE_EXTENSION};
use reth_node_builder::NodeTypesWithDBAdapter;
use reth_provider::StaticFileProviderFactory;
use reth_static_file_types::{
    ChangesetOffsetReader, ChangesetOffsetWriter, SegmentHeader, SegmentRangeInclusive,
    StaticFileSegment,
};
use std::path::{Path, PathBuf};
use tracing::info;

use crate::common::CliNodeTypes;

/// Reads storage changesets from static files and writes new static files with hashed storage keys.
///
/// The output files use unhashed addresses with keccak256-hashed storage slots.
#[derive(Parser, Debug)]
pub struct Command {
    /// Output directory for the migrated static files.
    #[arg(long, value_name = "OUTPUT_DIR")]
    output_dir: PathBuf,
}

impl Command {
    /// Execute `db hash-storage-changesets` command
    pub fn execute<N: CliNodeTypes>(
        self,
        tool: &DbTool<NodeTypesWithDBAdapter<N, DatabaseEnv>>,
    ) -> eyre::Result<()> {
        let static_file_provider = tool.provider_factory.static_file_provider();
        let input_dir = static_file_provider.directory().to_path_buf();

        reth_fs_util::create_dir_all(&self.output_dir)
            .map_err(|e| eyre::eyre!("Failed to create output directory: {e}"))?;

        let static_files = iter_static_files(&input_dir)?;
        let ranges = static_files
            .get(StaticFileSegment::StorageChangeSets)
            .ok_or_else(|| eyre::eyre!("No storage changeset static files found in {input_dir:?}"))?
            .clone();

        info!(
            jars = ranges.len(),
            input = ?input_dir,
            output = ?self.output_dir,
            "Starting storage changeset migration"
        );

        ranges.par_iter().try_for_each(|(_, header)| {
            migrate_jar(&input_dir, &self.output_dir, header.expected_block_range())
        })?;

        info!("Migration complete");

        Ok(())
    }
}

fn migrate_jar(
    input_dir: &Path,
    output_dir: &Path,
    block_range: SegmentRangeInclusive,
) -> eyre::Result<()> {
    let segment = StaticFileSegment::StorageChangeSets;
    let input_filename = segment.filename(&block_range);
    let input_path = input_dir.join(&input_filename);

    let source_jar = NippyJar::<SegmentHeader>::load(&input_path)?;
    let header = source_jar.user_header().clone();
    let total_rows = source_jar.rows();

    if total_rows == 0 {
        info!(range = %block_range, "Skipping empty jar");
        return Ok(());
    }

    let output_path = output_dir.join(&input_filename);
    let out_jar = NippyJar::new(
        segment.columns(),
        &output_path,
        SegmentHeader::new(header.expected_block_range(), None, None, segment),
    );
    let mut writer = NippyJarWriter::new(out_jar)?;

    let csoff_path = input_path.with_extension(CHANGESET_OFFSETS_FILE_EXTENSION);
    let len = header.changeset_offsets_len();
    let mut csoff_reader = ChangesetOffsetReader::new(&csoff_path, len)?;
    let changeset_offsets = csoff_reader.get_range(0, len)?;

    let out_csoff_path = output_path.with_extension(CHANGESET_OFFSETS_FILE_EXTENSION);
    let mut csoff_writer = ChangesetOffsetWriter::new(&out_csoff_path, 0)?;

    let reader = reth_nippy_jar::DataReader::new(&input_path)?;
    let mut cursor =
        reth_nippy_jar::NippyJarCursor::with_reader(&source_jar, std::sync::Arc::new(reader))?;

    let mut buf = Vec::with_capacity(128);
    let mut rows_written = 0u64;

    for offset in &changeset_offsets {
        let num_changes = offset.num_changes();
        let cs_offset = reth_static_file_types::ChangesetOffset::new(rows_written, num_changes);

        for _ in 0..num_changes {
            let row = cursor
                .next_row()?
                .ok_or_else(|| eyre::eyre!("Unexpected end of rows in {input_filename}"))?;

            let change = StorageBeforeTx::decompress(row[0])?;

            let hashed = StorageBeforeTx {
                address: change.address,
                key: keccak256(change.key),
                value: change.value,
            };

            buf.clear();
            hashed.to_compact(&mut buf);
            writer.append_column(Some(Ok(&buf)))?;

            rows_written += 1;
        }

        writer.user_header_mut().increment_block();
        csoff_writer.append(&cs_offset)?;
    }

    csoff_writer.sync()?;
    writer.commit()?;

    info!(
        range = %block_range,
        rows = rows_written,
        "Migrated jar"
    );

    Ok(())
}

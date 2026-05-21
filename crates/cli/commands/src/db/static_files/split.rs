use clap::Parser;
use reth_codecs::Compact;
use reth_db::{
    cursor::DbCursorRO,
    static_file::{
        AccountChangesetMask, BlockHashMask, HeaderMask, ReceiptMask, StorageChangesetMask,
        TotalDifficultyMask, TransactionMask, TransactionSenderMask,
    },
    tables,
    transaction::DbTx,
};
use reth_db_api::models::{CompactU256, StoredBlockBodyIndices};
use reth_db_common::DbTool;
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    providers::{ProviderNodeTypes, StaticFileProvider},
    DBProvider, StaticFileProviderBuilder, StaticFileProviderFactory, StaticFileWriter,
};
use reth_static_file_types::StaticFileSegment;
use std::{collections::HashMap, path::PathBuf};
use tracing::info;

/// Split static files into new files with different blocks-per-file setting
#[derive(Debug, Parser)]
pub struct SplitCommand {
    /// Source static files directory.
    /// If not specified, uses the datadir's static_files directory.
    #[arg(long, value_name = "PATH")]
    static_files_dir: Option<PathBuf>,

    /// Output directory for the new static files.
    /// Required unless --in-place is specified.
    #[arg(long, value_name = "PATH", required_unless_present = "in_place")]
    output_dir: Option<PathBuf>,

    /// Number of blocks per output file
    #[arg(long, value_name = "NUM")]
    blocks_per_file: u64,

    /// Segments to split (default: all)
    #[arg(long, value_delimiter = ',')]
    segments: Option<Vec<StaticFileSegment>>,

    /// Start block number (default: 0)
    #[arg(long)]
    from_block: Option<u64>,

    /// End block number (default: highest available)
    #[arg(long)]
    to_block: Option<u64>,

    /// Print what would be done without writing
    #[arg(long)]
    dry_run: bool,

    /// Split in-place: write to temp dir, verify, then atomically swap.
    /// Original files are preserved in static_files.bak
    #[arg(long, conflicts_with = "output_dir")]
    in_place: bool,

    /// Skip verification step when using --in-place
    #[arg(long, requires = "in_place")]
    skip_verify: bool,
}

impl SplitCommand {
    /// Execute the split command
    pub fn execute<N: ProviderNodeTypes>(self, tool: &DbTool<N>) -> eyre::Result<()>
    where
        N::Primitives: NodePrimitives<BlockHeader: Compact, SignedTx: Compact, Receipt: Compact>,
    {
        let segments = self.segments.clone().unwrap_or_else(|| StaticFileSegment::iter().collect());

        // Use custom static files dir if provided, otherwise use datadir's static files
        let (source_provider, source_dir) =
            if let Some(ref static_files_dir) = self.static_files_dir {
                let provider = StaticFileProviderBuilder::read_only(static_files_dir)
                    .build::<N::Primitives>()?;
                let dir = static_files_dir.clone();
                (provider, dir)
            } else {
                let provider = tool.provider_factory.static_file_provider();
                let dir = provider.directory().to_path_buf();
                (provider, dir)
            };

        // Determine output directory
        let (output_dir, is_in_place) = if self.in_place {
            let temp_dir = source_dir.with_file_name("static_files.tmp");
            (temp_dir, true)
        } else {
            (self.output_dir.clone().expect("output_dir required when not in_place"), false)
        };

        info!(
            target: "reth::cli",
            output_dir = %output_dir.display(),
            blocks_per_file = self.blocks_per_file,
            ?segments,
            from_block = ?self.from_block,
            to_block = ?self.to_block,
            dry_run = self.dry_run,
            in_place = is_in_place,
            "Splitting static files"
        );

        if self.dry_run {
            println!("Dry run mode - no files will be written");
            if is_in_place {
                println!("In-place mode:");
                println!("  1. Write to: {}", output_dir.display());
                println!("  2. Verify output integrity");
                println!("  3. Rename {} -> {}.bak", source_dir.display(), source_dir.display());
                println!("  4. Rename {} -> {}", output_dir.display(), source_dir.display());
            }
            for segment in &segments {
                let min_block = source_provider.get_lowest_range_start(*segment);
                let max_block = source_provider.get_highest_static_file_block(*segment);
                if let (Some(min_block), Some(max_block)) = (min_block, max_block) {
                    let from_block = self.from_block.unwrap_or(min_block).max(min_block);
                    let to_block = self.to_block.unwrap_or(max_block).min(max_block);
                    let num_blocks = to_block.saturating_sub(from_block) + 1;
                    let num_files = num_blocks.div_ceil(self.blocks_per_file);
                    println!(
                        "  {segment}: blocks {from_block}..={to_block} ({num_blocks} blocks) -> {num_files} files"
                    );
                } else {
                    println!("  {segment}: no data available");
                }
            }
            return Ok(());
        }

        // Clean up output directory if it exists
        // For in-place mode: remove previous incomplete temp directory
        // For regular mode: ensure we start fresh to avoid block number mismatches
        if output_dir.exists() {
            info!(target: "reth::cli", output_dir = %output_dir.display(), "Removing existing output directory");
            reth_fs_util::remove_dir_all(&output_dir)?;
        }

        reth_fs_util::create_dir_all(&output_dir)?;

        // Calculate segment ranges first to determine the global starting block
        let mut segment_ranges = Vec::new();
        for &segment in &segments {
            let Some(min_block) = source_provider.get_lowest_range_start(segment) else {
                continue;
            };
            let Some(max_block) = source_provider.get_highest_static_file_block(segment) else {
                continue;
            };
            let from_block = self.from_block.unwrap_or(min_block).max(min_block);
            let to_block = self.to_block.unwrap_or(max_block).min(max_block);
            if from_block <= to_block {
                segment_ranges.push((segment, from_block, to_block));
            }
        }

        // Pre-load block body indices for segments that need them (transactions, receipts,
        // transaction senders). This avoids holding a long-lived DB read transaction open and
        // is much faster than seeking per-block since the entire table is small.
        let needs_indices = segment_ranges.iter().any(|(seg, _, _)| {
            matches!(
                seg,
                StaticFileSegment::Transactions |
                    StaticFileSegment::Receipts |
                    StaticFileSegment::TransactionSenders
            )
        });
        let block_body_indices = if needs_indices {
            let global_from = segment_ranges
                .iter()
                .filter(|(seg, _, _)| {
                    matches!(
                        seg,
                        StaticFileSegment::Transactions |
                            StaticFileSegment::Receipts |
                            StaticFileSegment::TransactionSenders
                    )
                })
                .map(|(_, from, _)| *from)
                .min()
                .unwrap();
            let global_to = segment_ranges
                .iter()
                .filter(|(seg, _, _)| {
                    matches!(
                        seg,
                        StaticFileSegment::Transactions |
                            StaticFileSegment::Receipts |
                            StaticFileSegment::TransactionSenders
                    )
                })
                .map(|(_, _, to)| *to)
                .max()
                .unwrap();

            info!(target: "reth::cli", from_block = global_from, to_block = global_to, "Loading block body indices");
            Self::load_block_body_indices(tool, global_from, global_to)?
        } else {
            HashMap::new()
        };

        for (segment, from_block, to_block) in segment_ranges {
            info!(target: "reth::cli", ?segment, from_block, to_block, "Processing segment");

            // Build output provider per-segment with genesis_block_number set to this segment's
            // starting block. This prevents the writer from trying to load non-existent previous
            // files when segments have different starting blocks (e.g., pruned transactions).
            let output_provider = StaticFileProviderBuilder::read_write(&output_dir)
                .with_blocks_per_file(self.blocks_per_file)
                .with_genesis_block_number(from_block)
                .build::<N::Primitives>()?;

            match segment {
                StaticFileSegment::Headers => {
                    self.split_headers::<N>(
                        &source_provider,
                        &output_provider,
                        from_block,
                        to_block,
                    )?;
                }
                StaticFileSegment::Transactions => {
                    self.split_transactions::<N>(
                        &block_body_indices,
                        &source_provider,
                        &output_provider,
                        from_block,
                        to_block,
                    )?;
                }
                StaticFileSegment::Receipts => {
                    self.split_receipts::<N>(
                        &block_body_indices,
                        &source_provider,
                        &output_provider,
                        from_block,
                        to_block,
                    )?;
                }
                StaticFileSegment::TransactionSenders => {
                    self.split_transaction_senders::<N>(
                        &block_body_indices,
                        &source_provider,
                        &output_provider,
                        from_block,
                        to_block,
                    )?;
                }
                StaticFileSegment::AccountChangeSets => {
                    self.split_account_changesets::<N>(
                        &source_provider,
                        &output_provider,
                        from_block,
                        to_block,
                    )?;
                }
                StaticFileSegment::StorageChangeSets => {
                    self.split_storage_changesets::<N>(
                        &source_provider,
                        &output_provider,
                        from_block,
                        to_block,
                    )?;
                }
            }

            info!(target: "reth::cli", ?segment, "Segment complete");

            // Drop the output provider to release file handles before processing next segment
            drop(output_provider);
        }

        // In-place mode: verify and swap directories
        if is_in_place {
            // Verification step
            if !self.skip_verify {
                info!(target: "reth::cli", "Verifying output integrity");
                self.verify_output::<N>(&output_dir, &segments)?;
            }

            // Atomic swap
            let backup_dir = source_dir.with_file_name("static_files.bak");

            // Remove old backup if exists
            if backup_dir.exists() {
                info!(target: "reth::cli", backup_dir = %backup_dir.display(), "Removing old backup");
                reth_fs_util::remove_dir_all(&backup_dir)?;
            }

            // Drop source provider to release file handles
            drop(source_provider);

            // Rename: source -> backup
            info!(target: "reth::cli",
                from = %source_dir.display(),
                to = %backup_dir.display(),
                "Moving original to backup"
            );
            reth_fs_util::rename(&source_dir, &backup_dir)?;

            // Rename: temp -> source
            info!(target: "reth::cli",
                from = %output_dir.display(),
                to = %source_dir.display(),
                "Moving new files into place"
            );
            reth_fs_util::rename(&output_dir, &source_dir)?;

            info!(target: "reth::cli",
                backup = %backup_dir.display(),
                "In-place split complete. Original files preserved in backup directory"
            );
        }

        info!(target: "reth::cli", "Static file split complete");
        Ok(())
    }

    /// Verify the output static files have valid data
    fn verify_output<N: ProviderNodeTypes>(
        &self,
        output_dir: &PathBuf,
        segments: &[StaticFileSegment],
    ) -> eyre::Result<()> {
        let provider = StaticFileProviderBuilder::read_only(output_dir).build::<N::Primitives>()?;

        for &segment in segments {
            let Some(lowest) = provider.get_lowest_range_start(segment) else {
                return Err(eyre::eyre!("Verification failed: no data for segment {segment}"));
            };
            let Some(highest) = provider.get_highest_static_file_block(segment) else {
                return Err(eyre::eyre!("Verification failed: no data for segment {segment}"));
            };

            // Verify we can read the first and last blocks
            provider.get_segment_provider(segment, lowest)?;
            provider.get_segment_provider(segment, highest)?;

            info!(target: "reth::cli", ?segment, from_block = lowest, to_block = highest, "Verified");
        }

        Ok(())
    }

    fn split_headers<N: ProviderNodeTypes>(
        &self,
        source: &StaticFileProvider<N::Primitives>,
        output: &StaticFileProvider<N::Primitives>,
        from_block: u64,
        to_block: u64,
    ) -> eyre::Result<()>
    where
        <N::Primitives as NodePrimitives>::BlockHeader: Compact,
    {
        let mut writer = output.get_writer(from_block, StaticFileSegment::Headers)?;

        for block in from_block..=to_block {
            let jar = source.get_segment_provider(StaticFileSegment::Headers, block)?;
            let mut cursor = jar.cursor()?;

            let header: <N::Primitives as NodePrimitives>::BlockHeader = cursor
                .get_one::<HeaderMask<_>>(block.into())?
                .ok_or_else(|| eyre::eyre!("Missing header for block {block}"))?;

            let td: CompactU256 = cursor
                .get_one::<TotalDifficultyMask>(block.into())?
                .ok_or_else(|| eyre::eyre!("Missing TD for block {block}"))?;

            let hash = cursor
                .get_one::<BlockHashMask>(block.into())?
                .ok_or_else(|| eyre::eyre!("Missing hash for block {block}"))?;

            writer.append_header_with_td(&header, td.into(), &hash)?;

            if block % 100_000 == 0 {
                info!(target: "reth::cli", block, to_block, "Headers progress");
            }
        }

        writer.commit()?;
        Ok(())
    }

    fn load_block_body_indices<N: ProviderNodeTypes>(
        tool: &DbTool<N>,
        from_block: u64,
        to_block: u64,
    ) -> eyre::Result<HashMap<u64, StoredBlockBodyIndices>> {
        let provider = tool.provider_factory.provider()?.disable_long_read_transaction_safety();
        let tx = provider.tx_ref();
        let mut cursor = tx.cursor_read::<tables::BlockBodyIndices>()?;
        let mut indices = HashMap::with_capacity((to_block - from_block + 1) as usize);
        for entry in cursor.walk_range(from_block..=to_block)? {
            let (block, body_indices) = entry?;
            indices.insert(block, body_indices);
        }
        info!(target: "reth::cli", count = indices.len(), "Loaded block body indices");
        Ok(indices)
    }

    fn split_transactions<N: ProviderNodeTypes>(
        &self,
        block_body_indices: &HashMap<u64, StoredBlockBodyIndices>,
        source: &StaticFileProvider<N::Primitives>,
        output: &StaticFileProvider<N::Primitives>,
        from_block: u64,
        to_block: u64,
    ) -> eyre::Result<()>
    where
        <N::Primitives as NodePrimitives>::SignedTx: Compact,
    {
        let mut writer = output.get_writer(from_block, StaticFileSegment::Transactions)?;
        let mut block = from_block;
        let mut block_incremented = false;

        while block <= to_block {
            if !block_incremented {
                writer.increment_block(block)?;
            }
            block_incremented = false;

            // Skip blocks with no transactions until we find one that needs a jar
            let Some(indices) =
                block_body_indices.get(&block).filter(|i| i.tx_count > 0)
            else {
                if block.is_multiple_of(100_000) {
                    info!(target: "reth::cli", block, to_block, "Transactions progress");
                }
                block += 1;
                continue;
            };

            // Open jar + cursor, reuse for all subsequent blocks within this jar's range
            let jar =
                source.get_segment_provider(StaticFileSegment::Transactions, indices.first_tx_num)?;
            let jar_tx_end =
                jar.user_header().tx_range().map(|r| r.end()).unwrap_or(u64::MAX);
            let mut cursor = jar.cursor()?;

            loop {
                if let Some(indices) = block_body_indices.get(&block) {
                    for tx_num in indices.first_tx_num..indices.first_tx_num + indices.tx_count {
                        let transaction: <N::Primitives as NodePrimitives>::SignedTx = cursor
                            .get_one::<TransactionMask<_>>(tx_num.into())?
                            .ok_or_else(|| eyre::eyre!("Missing transaction {tx_num}"))?;
                        writer.append_transaction(tx_num, &transaction)?;
                    }
                }

                if block.is_multiple_of(100_000) {
                    info!(target: "reth::cli", block, to_block, "Transactions progress");
                }
                block += 1;
                if block > to_block {
                    break;
                }

                writer.increment_block(block)?;
                block_incremented = true;

                // Check if next block's txs need a different jar
                if let Some(next_indices) = block_body_indices.get(&block) &&
                    next_indices.tx_count > 0 && next_indices.first_tx_num > jar_tx_end
                {
                    break;
                }
            }
        }

        writer.commit()?;
        Ok(())
    }

    fn split_receipts<N: ProviderNodeTypes>(
        &self,
        block_body_indices: &HashMap<u64, StoredBlockBodyIndices>,
        source: &StaticFileProvider<N::Primitives>,
        output: &StaticFileProvider<N::Primitives>,
        from_block: u64,
        to_block: u64,
    ) -> eyre::Result<()>
    where
        <N::Primitives as NodePrimitives>::Receipt: Compact,
    {
        let mut writer = output.get_writer(from_block, StaticFileSegment::Receipts)?;
        let mut block = from_block;
        let mut block_incremented = false;

        while block <= to_block {
            if !block_incremented {
                writer.increment_block(block)?;
            }
            block_incremented = false;

            let Some(indices) =
                block_body_indices.get(&block).filter(|i| i.tx_count > 0)
            else {
                if block.is_multiple_of(100_000) {
                    info!(target: "reth::cli", block, to_block, "Receipts progress");
                }
                block += 1;
                continue;
            };

            let jar =
                source.get_segment_provider(StaticFileSegment::Receipts, indices.first_tx_num)?;
            let jar_tx_end =
                jar.user_header().tx_range().map(|r| r.end()).unwrap_or(u64::MAX);
            let mut cursor = jar.cursor()?;

            loop {
                if let Some(indices) = block_body_indices.get(&block) {
                    for tx_num in indices.first_tx_num..indices.first_tx_num + indices.tx_count {
                        let receipt: <N::Primitives as NodePrimitives>::Receipt = cursor
                            .get_one::<ReceiptMask<_>>(tx_num.into())?
                            .ok_or_else(|| eyre::eyre!("Missing receipt {tx_num}"))?;
                        writer.append_receipt(tx_num, &receipt)?;
                    }
                }

                if block.is_multiple_of(100_000) {
                    info!(target: "reth::cli", block, to_block, "Receipts progress");
                }
                block += 1;
                if block > to_block {
                    break;
                }

                writer.increment_block(block)?;
                block_incremented = true;

                if let Some(next_indices) = block_body_indices.get(&block) &&
                    next_indices.tx_count > 0 && next_indices.first_tx_num > jar_tx_end
                {
                    break;
                }
            }
        }

        writer.commit()?;
        Ok(())
    }

    fn split_transaction_senders<N: ProviderNodeTypes>(
        &self,
        block_body_indices: &HashMap<u64, StoredBlockBodyIndices>,
        source: &StaticFileProvider<N::Primitives>,
        output: &StaticFileProvider<N::Primitives>,
        from_block: u64,
        to_block: u64,
    ) -> eyre::Result<()> {
        let mut writer = output.get_writer(from_block, StaticFileSegment::TransactionSenders)?;
        let mut block = from_block;
        let mut block_incremented = false;

        while block <= to_block {
            if !block_incremented {
                writer.increment_block(block)?;
            }
            block_incremented = false;

            let Some(indices) =
                block_body_indices.get(&block).filter(|i| i.tx_count > 0)
            else {
                if block.is_multiple_of(100_000) {
                    info!(target: "reth::cli", block, to_block, "Transaction senders progress");
                }
                block += 1;
                continue;
            };

            let jar = source
                .get_segment_provider(StaticFileSegment::TransactionSenders, indices.first_tx_num)?;
            let jar_tx_end =
                jar.user_header().tx_range().map(|r| r.end()).unwrap_or(u64::MAX);
            let mut cursor = jar.cursor()?;

            loop {
                if let Some(indices) = block_body_indices.get(&block) {
                    for tx_num in indices.first_tx_num..indices.first_tx_num + indices.tx_count {
                        let sender = cursor
                            .get_one::<TransactionSenderMask>(tx_num.into())?
                            .ok_or_else(|| eyre::eyre!("Missing sender {tx_num}"))?;
                        writer.append_transaction_sender(tx_num, &sender)?;
                    }
                }

                if block.is_multiple_of(100_000) {
                    info!(target: "reth::cli", block, to_block, "Transaction senders progress");
                }
                block += 1;
                if block > to_block {
                    break;
                }

                writer.increment_block(block)?;
                block_incremented = true;

                if let Some(next_indices) = block_body_indices.get(&block) &&
                    next_indices.tx_count > 0 && next_indices.first_tx_num > jar_tx_end
                {
                    break;
                }
            }
        }

        writer.commit()?;
        Ok(())
    }

    fn split_account_changesets<N: ProviderNodeTypes>(
        &self,
        source: &StaticFileProvider<N::Primitives>,
        output: &StaticFileProvider<N::Primitives>,
        from_block: u64,
        to_block: u64,
    ) -> eyre::Result<()> {
        let mut writer = output.get_writer(from_block, StaticFileSegment::AccountChangeSets)?;
        let mut block = from_block;

        while block <= to_block {
            // Open jar + cursor, reuse for all blocks within this jar's range
            let jar =
                source.get_segment_provider(StaticFileSegment::AccountChangeSets, block)?;
            let jar_block_end = jar
                .user_header()
                .block_range()
                .map(|r| r.end())
                .unwrap_or(u64::MAX);
            let mut cursor = jar.cursor()?;

            loop {
                let mut changes = Vec::new();
                if let Some(offset) = jar.read_changeset_offset(block)? {
                    for i in offset.changeset_range() {
                        if let Some(change) =
                            cursor.get_one::<AccountChangesetMask>(i.into())?
                        {
                            changes.push(change);
                        }
                    }
                }

                writer.append_account_changeset(changes, block)?;

                if block.is_multiple_of(100_000) {
                    info!(target: "reth::cli", block, to_block, "Account changesets progress");
                }
                block += 1;
                if block > to_block || block > jar_block_end {
                    break;
                }
            }
        }

        writer.commit()?;
        Ok(())
    }

    fn split_storage_changesets<N: ProviderNodeTypes>(
        &self,
        source: &StaticFileProvider<N::Primitives>,
        output: &StaticFileProvider<N::Primitives>,
        from_block: u64,
        to_block: u64,
    ) -> eyre::Result<()> {
        let mut writer = output.get_writer(from_block, StaticFileSegment::StorageChangeSets)?;
        let mut block = from_block;

        while block <= to_block {
            let jar =
                source.get_segment_provider(StaticFileSegment::StorageChangeSets, block)?;
            let jar_block_end = jar
                .user_header()
                .block_range()
                .map(|r| r.end())
                .unwrap_or(u64::MAX);
            let mut cursor = jar.cursor()?;

            loop {
                let mut changes = Vec::new();
                if let Some(offset) = jar.read_changeset_offset(block)? {
                    for i in offset.changeset_range() {
                        if let Some(change) =
                            cursor.get_one::<StorageChangesetMask>(i.into())?
                        {
                            changes.push(change);
                        }
                    }
                }

                writer.append_storage_changeset(changes, block)?;

                if block.is_multiple_of(100_000) {
                    info!(target: "reth::cli", block, to_block, "Storage changesets progress");
                }
                block += 1;
                if block > to_block || block > jar_block_end {
                    break;
                }
            }
        }

        writer.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: TestCommand,
    }

    #[derive(clap::Subcommand)]
    enum TestCommand {
        Split(SplitCommand),
    }

    #[test]
    fn parse_split_command_minimal() {
        let args = TestCli::try_parse_from([
            "test",
            "split",
            "--output-dir",
            "/tmp/output",
            "--blocks-per-file",
            "100000",
        ])
        .unwrap();

        match args.command {
            TestCommand::Split(cmd) => {
                assert_eq!(cmd.output_dir, Some(PathBuf::from("/tmp/output")));
                assert_eq!(cmd.blocks_per_file, 100000);
                assert!(cmd.segments.is_none());
                assert!(cmd.from_block.is_none());
                assert!(cmd.to_block.is_none());
                assert!(!cmd.dry_run);
                assert!(!cmd.in_place);
            }
        }
    }

    #[test]
    fn parse_split_command_full() {
        let args = TestCli::try_parse_from([
            "test",
            "split",
            "--output-dir",
            "/tmp/output",
            "--blocks-per-file",
            "50000",
            "--segments",
            "headers,receipts",
            "--from-block",
            "1000",
            "--to-block",
            "500000",
            "--dry-run",
        ])
        .unwrap();

        match args.command {
            TestCommand::Split(cmd) => {
                assert_eq!(cmd.output_dir, Some(PathBuf::from("/tmp/output")));
                assert_eq!(cmd.blocks_per_file, 50000);
                assert_eq!(
                    cmd.segments,
                    Some(vec![StaticFileSegment::Headers, StaticFileSegment::Receipts])
                );
                assert_eq!(cmd.from_block, Some(1000));
                assert_eq!(cmd.to_block, Some(500000));
                assert!(cmd.dry_run);
                assert!(!cmd.in_place);
            }
        }
    }

    #[test]
    fn parse_split_command_in_place() {
        let args =
            TestCli::try_parse_from(["test", "split", "--in-place", "--blocks-per-file", "100000"])
                .unwrap();

        match args.command {
            TestCommand::Split(cmd) => {
                assert!(cmd.output_dir.is_none());
                assert_eq!(cmd.blocks_per_file, 100000);
                assert!(cmd.in_place);
                assert!(!cmd.skip_verify);
            }
        }
    }

    #[test]
    fn parse_split_command_in_place_skip_verify() {
        let args = TestCli::try_parse_from([
            "test",
            "split",
            "--in-place",
            "--skip-verify",
            "--blocks-per-file",
            "100000",
        ])
        .unwrap();

        match args.command {
            TestCommand::Split(cmd) => {
                assert!(cmd.in_place);
                assert!(cmd.skip_verify);
            }
        }
    }

    #[test]
    fn parse_split_command_output_dir_conflicts_with_in_place() {
        let result = TestCli::try_parse_from([
            "test",
            "split",
            "--output-dir",
            "/tmp/out",
            "--in-place",
            "--blocks-per-file",
            "100000",
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn parse_split_command_skip_verify_requires_in_place() {
        // --skip-verify without --in-place should fail
        let result = TestCli::try_parse_from([
            "test",
            "split",
            "--skip-verify",
            "--blocks-per-file",
            "100000",
        ]);
        assert!(result.is_err(), "--skip-verify should require --in-place");
    }

    #[test]
    fn parse_split_command_all_segments() {
        let args = TestCli::try_parse_from([
            "test",
            "split",
            "--output-dir",
            "/tmp/out",
            "--blocks-per-file",
            "10",
            "--segments",
            "headers,transactions,receipts,transaction-senders,account-change-sets,storage-change-sets",
        ])
        .unwrap();

        match args.command {
            TestCommand::Split(cmd) => {
                let segments = cmd.segments.unwrap();
                assert_eq!(segments.len(), 6);
                assert!(segments.contains(&StaticFileSegment::Headers));
                assert!(segments.contains(&StaticFileSegment::Transactions));
                assert!(segments.contains(&StaticFileSegment::Receipts));
                assert!(segments.contains(&StaticFileSegment::TransactionSenders));
                assert!(segments.contains(&StaticFileSegment::AccountChangeSets));
                assert!(segments.contains(&StaticFileSegment::StorageChangeSets));
            }
        }
    }
}

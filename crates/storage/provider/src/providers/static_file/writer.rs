use super::{
    manager::StaticFileProviderInner, metrics::StaticFileProviderMetrics, StaticFileProvider,
};
use crate::providers::static_file::metrics::StaticFileProviderOperation;
use alloy_consensus::BlockHeader;
use alloy_primitives::{BlockHash, BlockNumber, TxNumber, U256};
use parking_lot::{lock_api::RwLockWriteGuard, RawRwLock, RwLock};
use reth_codecs::Compact;
use reth_db::models::{AccountBeforeTx, StorageBeforeTx};
use reth_db_api::models::CompactU256;
use reth_nippy_jar::{NippyJar, NippyJarError, NippyJarWriter};
use reth_node_types::NodePrimitives;
use reth_static_file_types::{
    ChangesetOffset, ChangesetOffsetReader, ChangesetOffsetWriter, SegmentHeader,
    SegmentRangeInclusive, StaticFileSegment,
};
use reth_storage_errors::provider::{ProviderError, ProviderResult, StaticFileWriterError};
use std::{
    borrow::Borrow,
    cmp::Ordering,
    fmt::Debug,
    path::{Path, PathBuf},
    sync::{Arc, Weak},
    time::Instant,
};
use tracing::{debug, instrument};

/// Represents different pruning strategies for various static file segments.
#[derive(Debug, Clone, Copy)]
enum PruneStrategy {
    /// Prune headers by number of blocks to delete.
    Headers {
        /// Number of blocks to delete.
        num_blocks: u64,
    },
    /// Prune transactions by number of rows and last block.
    Transactions {
        /// Number of transaction rows to delete.
        num_rows: u64,
        /// The last block number after pruning.
        last_block: BlockNumber,
    },
    /// Prune receipts by number of rows and last block.
    Receipts {
        /// Number of receipt rows to delete.
        num_rows: u64,
        /// The last block number after pruning.
        last_block: BlockNumber,
    },
    /// Prune transaction senders by number of rows and last block.
    TransactionSenders {
        /// Number of transaction sender rows to delete.
        num_rows: u64,
        /// The last block number after pruning.
        last_block: BlockNumber,
    },
    /// Prune account changesets to a target block number.
    AccountChangeSets {
        /// The target block number to prune to.
        last_block: BlockNumber,
    },
    /// Prune storage changesets to a target block number.
    StorageChangeSets {
        /// The target block number to prune to.
        last_block: BlockNumber,
    },
}

/// Static file writers for every known [`StaticFileSegment`].
///
/// WARNING: Trying to use more than one writer for the same segment type **will result in a
/// deadlock**.
#[derive(Debug)]
pub(crate) struct StaticFileWriters<N> {
    headers: RwLock<Option<StaticFileProviderRW<N>>>,
    transactions: RwLock<Option<StaticFileProviderRW<N>>>,
    receipts: RwLock<Option<StaticFileProviderRW<N>>>,
    transaction_senders: RwLock<Option<StaticFileProviderRW<N>>>,
    account_change_sets: RwLock<Option<StaticFileProviderRW<N>>>,
    storage_change_sets: RwLock<Option<StaticFileProviderRW<N>>>,
}

impl<N> Default for StaticFileWriters<N> {
    fn default() -> Self {
        Self {
            headers: Default::default(),
            transactions: Default::default(),
            receipts: Default::default(),
            transaction_senders: Default::default(),
            account_change_sets: Default::default(),
            storage_change_sets: Default::default(),
        }
    }
}

impl<N: NodePrimitives> StaticFileWriters<N> {
    pub(crate) fn get_or_create(
        &self,
        segment: StaticFileSegment,
        create_fn: impl FnOnce() -> ProviderResult<StaticFileProviderRW<N>>,
    ) -> ProviderResult<StaticFileProviderRWRefMut<'_, N>> {
        let mut write_guard = match segment {
            StaticFileSegment::Headers => self.headers.write(),
            StaticFileSegment::Transactions => self.transactions.write(),
            StaticFileSegment::Receipts => self.receipts.write(),
            StaticFileSegment::TransactionSenders => self.transaction_senders.write(),
            StaticFileSegment::AccountChangeSets => self.account_change_sets.write(),
            StaticFileSegment::StorageChangeSets => self.storage_change_sets.write(),
        };

        if write_guard.is_none() {
            *write_guard = Some(create_fn()?);
        }

        Ok(StaticFileProviderRWRefMut(write_guard))
    }

    #[instrument(
        name = "StaticFileWriters::commit",
        level = "debug",
        target = "providers::static_file",
        skip_all
    )]
    pub(crate) fn commit(&self) -> ProviderResult<()> {
        debug!(target: "providers::static_file", "Committing all static file segments");

        for writer_lock in [
            &self.headers,
            &self.transactions,
            &self.receipts,
            &self.transaction_senders,
            &self.account_change_sets,
            &self.storage_change_sets,
        ] {
            let mut writer = writer_lock.write();
            if let Some(writer) = writer.as_mut() {
                writer.commit()?;
            }
        }

        debug!(target: "providers::static_file", "Committed all static file segments");
        Ok(())
    }

    pub(crate) fn has_unwind_queued(&self) -> bool {
        for writer_lock in [
            &self.headers,
            &self.transactions,
            &self.receipts,
            &self.transaction_senders,
            &self.account_change_sets,
            &self.storage_change_sets,
        ] {
            let writer = writer_lock.read();
            if let Some(writer) = writer.as_ref() &&
                writer.will_prune_on_commit()
            {
                return true
            }
        }
        false
    }

    /// Finalizes all writers by committing their configuration to disk and updating indices.
    ///
    /// Must be called after `sync_all` was called on individual writers.
    /// Returns an error if any writer has prune queued.
    #[instrument(
        name = "StaticFileWriters::finalize",
        level = "debug",
        target = "providers::static_file",
        skip_all
    )]
    pub(crate) fn finalize(&self) -> ProviderResult<()> {
        debug!(target: "providers::static_file", "Finalizing all static file segments into disk");

        for writer_lock in [
            &self.headers,
            &self.transactions,
            &self.receipts,
            &self.transaction_senders,
            &self.account_change_sets,
            &self.storage_change_sets,
        ] {
            let mut writer = writer_lock.write();
            if let Some(writer) = writer.as_mut() {
                writer.finalize()?;
            }
        }

        debug!(target: "providers::static_file", "Finalized all static file segments into disk");
        Ok(())
    }
}

/// Mutable reference to a [`StaticFileProviderRW`] behind a [`RwLockWriteGuard`].
#[derive(Debug)]
pub struct StaticFileProviderRWRefMut<'a, N>(
    pub(crate) RwLockWriteGuard<'a, RawRwLock, Option<StaticFileProviderRW<N>>>,
);

impl<N> std::ops::DerefMut for StaticFileProviderRWRefMut<'_, N> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // This is always created by [`StaticFileWriters::get_or_create`]
        self.0.as_mut().expect("static file writer provider should be init")
    }
}

impl<N> std::ops::Deref for StaticFileProviderRWRefMut<'_, N> {
    type Target = StaticFileProviderRW<N>;

    fn deref(&self) -> &Self::Target {
        // This is always created by [`StaticFileWriters::get_or_create`]
        self.0.as_ref().expect("static file writer provider should be init")
    }
}

#[derive(Debug)]
/// Extends `StaticFileProvider` with writing capabilities
pub struct StaticFileProviderRW<N> {
    /// Reference back to the provider. We need [Weak] here because [`StaticFileProviderRW`] is
    /// stored in a [`reth_primitives_traits::dashmap::DashMap`] inside the parent
    /// [`StaticFileProvider`].which is an [Arc]. If we were to use an [Arc] here, we would
    /// create a reference cycle.
    reader: Weak<StaticFileProviderInner<N>>,
    /// A [`NippyJarWriter`] instance.
    writer: NippyJarWriter<SegmentHeader>,
    /// Path to opened file.
    data_path: PathBuf,
    /// Reusable buffer for encoding appended data.
    buf: Vec<u8>,
    /// Metrics.
    metrics: Option<Arc<StaticFileProviderMetrics>>,
    /// On commit, contains the pruning strategy to apply for the segment.
    prune_on_commit: Option<PruneStrategy>,
    /// Whether `sync_all()` has been called. Used by `finalize()` to avoid redundant syncs.
    synced: bool,
    /// Changeset offsets sidecar writer (only for changeset segments).
    changeset_offsets: Option<ChangesetOffsetWriter>,
    /// Current block's changeset offset being written.
    current_changeset_offset: Option<ChangesetOffset>,
}

impl<N: NodePrimitives> StaticFileProviderRW<N> {
    /// Creates a new [`StaticFileProviderRW`] for a [`StaticFileSegment`].
    ///
    /// Before use, transaction based segments should ensure the block end range is the expected
    /// one, and heal if not. For more check `Self::ensure_end_range_consistency`.
    pub fn new(
        segment: StaticFileSegment,
        block: BlockNumber,
        reader: Weak<StaticFileProviderInner<N>>,
        metrics: Option<Arc<StaticFileProviderMetrics>>,
    ) -> ProviderResult<Self> {
        let (writer, data_path) = Self::open(segment, block, reader.clone(), metrics.clone())?;

        // Create writer WITHOUT sidecar first - we'll add it after healing
        let mut writer = Self {
            writer,
            data_path,
            buf: Vec::with_capacity(100),
            reader,
            metrics,
            prune_on_commit: None,
            synced: false,
            changeset_offsets: None,
            current_changeset_offset: None,
        };

        // Run NippyJar healing BEFORE setting up changeset sidecar
        // This may reduce rows, which affects valid sidecar offsets
        writer.ensure_end_range_consistency()?;

        // Now set up changeset sidecar with post-heal header values
        if segment.is_change_based() {
            writer.heal_changeset_sidecar()?;
        }

        Ok(writer)
    }

    fn open(
        segment: StaticFileSegment,
        block: u64,
        reader: Weak<StaticFileProviderInner<N>>,
        metrics: Option<Arc<StaticFileProviderMetrics>>,
    ) -> ProviderResult<(NippyJarWriter<SegmentHeader>, PathBuf)> {
        let start = Instant::now();

        let static_file_provider = Self::upgrade_provider_to_strong_reference(&reader);

        let block_range = static_file_provider.find_fixed_range(segment, block);
        let (jar, path) = match static_file_provider.get_segment_provider_for_block(
            segment,
            block_range.start(),
            None,
        ) {
            Ok(provider) => (
                NippyJar::load(provider.data_path()).map_err(ProviderError::other)?,
                provider.data_path().into(),
            ),
            Err(ProviderError::MissingStaticFileBlock(_, _)) => {
                let path = static_file_provider.directory().join(segment.filename(&block_range));
                (create_jar(segment, &path, block_range), path)
            }
            Err(err) => return Err(err),
        };

        let result = match NippyJarWriter::new(jar) {
            Ok(writer) => Ok((writer, path)),
            Err(NippyJarError::FrozenJar) => {
                // This static file has been frozen, so we should
                Err(ProviderError::FinalizedStaticFile(segment, block))
            }
            Err(e) => Err(ProviderError::other(e)),
        }?;

        if let Some(metrics) = &metrics {
            metrics.record_segment_operation(
                segment,
                StaticFileProviderOperation::OpenWriter,
                Some(start.elapsed()),
            );
        }

        Ok(result)
    }

    /// If a file level healing happens, we need to update the end range on the
    /// [`SegmentHeader`].
    ///
    /// However, for transaction based segments, the block end range has to be found and healed
    /// externally.
    ///
    /// Check [`reth_nippy_jar::NippyJarChecker`] &
    /// [`NippyJarWriter`] for more on healing.
    fn ensure_end_range_consistency(&mut self) -> ProviderResult<()> {
        // If we have lost rows (in this run or previous), we need to update the [SegmentHeader].
        let expected_rows = if self.user_header().segment().is_headers() {
            self.user_header().block_len().unwrap_or_default()
        } else {
            self.user_header().tx_len().unwrap_or_default()
        };
        let actual_rows = self.writer.rows() as u64;
        let pruned_rows = expected_rows.saturating_sub(actual_rows);
        if pruned_rows > 0 {
            self.user_header_mut().prune(pruned_rows);
        }

        debug!(
            target: "providers::static_file",
            segment = ?self.writer.user_header().segment(),
            path = ?self.data_path,
            pruned_rows,
            "Ensuring end range consistency"
        );

        self.writer.commit().map_err(ProviderError::other)?;

        // Updates the [SnapshotProvider] manager
        self.update_index()?;
        Ok(())
    }

    /// Returns `true` if the writer will prune on commit.
    pub const fn will_prune_on_commit(&self) -> bool {
        self.prune_on_commit.is_some()
    }

    /// Heals the changeset offset sidecar after `NippyJar` healing.
    ///
    /// This must be called AFTER `ensure_end_range_consistency()` which may reduce rows.
    /// Performs three-way consistency check between header, `NippyJar` rows, and sidecar file:
    /// - Validates sidecar offsets don't point past actual `NippyJar` rows
    /// - Heals header if sidecar was truncated during interrupted prune
    /// - Truncates sidecar if offsets point past healed `NippyJar` data
    fn heal_changeset_sidecar(&mut self) -> ProviderResult<()> {
        let csoff_path = self.data_path.with_extension("csoff");

        // Step 1: Read all three sources of truth
        let header_claims_blocks = self.writer.user_header().changeset_offsets_len();
        let actual_nippy_rows = self.writer.rows() as u64;

        // Get actual sidecar file size (may differ from header after crash)
        let actual_sidecar_blocks = if csoff_path.exists() {
            let file_len = reth_fs_util::metadata(&csoff_path).map_err(ProviderError::other)?.len();
            // Remove partial records from crash mid-write
            let aligned_len = file_len - (file_len % 16);
            aligned_len / 16
        } else {
            0
        };

        // Fresh segment or no sidecar data - nothing to heal
        if header_claims_blocks == 0 && actual_sidecar_blocks == 0 {
            self.changeset_offsets =
                Some(ChangesetOffsetWriter::new(&csoff_path, 0).map_err(ProviderError::other)?);
            return Ok(());
        }

        // Step 2: Validate sidecar offsets against actual NippyJar state
        let valid_blocks = if actual_sidecar_blocks > 0 {
            let mut reader = ChangesetOffsetReader::new(&csoff_path, actual_sidecar_blocks)
                .map_err(ProviderError::other)?;

            // Find last block where offset + num_changes <= actual_nippy_rows
            // This correctly handles rows=0 with offset=0, num_changes=0 (empty blocks)
            let mut valid = 0u64;
            for i in 0..actual_sidecar_blocks {
                if let Some(offset) = reader.get(i).map_err(ProviderError::other)? {
                    if offset.offset() + offset.num_changes() <= actual_nippy_rows {
                        valid = i + 1;
                    } else {
                        // This block points past EOF - stop here
                        break;
                    }
                }
            }
            valid
        } else {
            0
        };

        // Step 3: Determine correct state from synced files (source of truth)
        // Header is the commit marker - never enlarge, only shrink
        let correct_blocks = valid_blocks.min(header_claims_blocks);

        // Step 4: Heal if header doesn't match validated truth
        let mut needs_header_commit = false;

        if correct_blocks != header_claims_blocks || actual_sidecar_blocks != correct_blocks {
            tracing::warn!(
                target: "reth::static_file",
                path = %csoff_path.display(),
                header_claims = header_claims_blocks,
                sidecar_has = actual_sidecar_blocks,
                valid_blocks = correct_blocks,
                actual_rows = actual_nippy_rows,
                "Three-way healing: syncing header, sidecar, and NippyJar state"
            );

            // Truncate sidecar file if it has invalid blocks
            if actual_sidecar_blocks > correct_blocks {
                use std::fs::OpenOptions;
                let file = OpenOptions::new()
                    .write(true)
                    .open(&csoff_path)
                    .map_err(ProviderError::other)?;
                file.set_len(correct_blocks * 16).map_err(ProviderError::other)?;
                file.sync_all().map_err(ProviderError::other)?;

                tracing::debug!(
                    target: "reth::static_file",
                    "Truncated sidecar from {} to {} blocks",
                    actual_sidecar_blocks,
                    correct_blocks
                );
            }

            // Update header to match validated truth (can only shrink, never enlarge)
            if correct_blocks < header_claims_blocks {
                // Blocks were removed - use prune() to update both block_range and
                // changeset_offsets_len atomically
                let blocks_removed = header_claims_blocks - correct_blocks;
                self.writer.user_header_mut().prune(blocks_removed);

                tracing::debug!(
                    target: "reth::static_file",
                    "Updated header: removed {} blocks (changeset_offsets_len: {} -> {})",
                    blocks_removed,
                    header_claims_blocks,
                    correct_blocks
                );

                needs_header_commit = true;
            }
        } else {
            tracing::debug!(
                target: "reth::static_file",
                path = %csoff_path.display(),
                blocks = correct_blocks,
                "Changeset sidecar consistent, no healing needed"
            );
        }

        // Open sidecar writer with corrected count (won't error now that sizes match)
        let csoff_writer = ChangesetOffsetWriter::new(&csoff_path, correct_blocks)
            .map_err(ProviderError::other)?;

        self.changeset_offsets = Some(csoff_writer);

        // Commit healed header if needed (after sidecar writer is set up)
        if needs_header_commit {
            self.writer.commit().map_err(ProviderError::other)?;

            tracing::info!(
                target: "reth::static_file",
                path = %csoff_path.display(),
                blocks = correct_blocks,
                "Committed healed changeset offset header"
            );
        }

        Ok(())
    }

    /// Flushes the current changeset offset (if any) to the `.csoff` sidecar file.
    ///
    /// This is idempotent - safe to call multiple times. After flushing, the current offset
    /// is cleared to prevent duplicate writes.
    ///
    /// This must be called before committing or syncing to ensure the last block's offset
    /// is persisted, since `increment_block()` only writes the *previous* block's offset.
    fn flush_current_changeset_offset(&mut self) -> ProviderResult<()> {
        if !self.writer.user_header().segment().is_change_based() {
            return Ok(());
        }

        if let Some(offset) = self.current_changeset_offset.take() &&
            let Some(writer) = &mut self.changeset_offsets
        {
            writer.append(&offset).map_err(ProviderError::other)?;
        }
        Ok(())
    }

    /// Syncs all data (rows, offsets, and changeset offsets sidecar) to disk.
    ///
    /// This does NOT commit the configuration. Call [`Self::finalize`] after to write the
    /// configuration and mark the writer as clean.
    ///
    /// Returns an error if prune is queued (use [`Self::commit`] instead).
    pub fn sync_all(&mut self) -> ProviderResult<()> {
        if self.prune_on_commit.is_some() {
            return Err(StaticFileWriterError::FinalizeWithPruneQueued.into());
        }

        // Write the final block's offset and sync the sidecar for changeset segments
        self.flush_current_changeset_offset()?;
        if let Some(writer) = &mut self.changeset_offsets {
            writer.sync().map_err(ProviderError::other)?;
            // Update the header with the actual number of offsets written
            self.writer.user_header_mut().set_changeset_offsets_len(writer.len());
        }

        if self.writer.is_dirty() {
            self.writer.sync_all().map_err(ProviderError::other)?;
        }
        self.synced = true;
        Ok(())
    }

    /// Commits configuration to disk and updates the reader index.
    ///
    /// If `sync_all()` was not called, this will call it first to ensure data is persisted.
    ///
    /// Returns an error if prune is queued (use [`Self::commit`] instead).
    #[instrument(
        name = "StaticFileProviderRW::finalize",
        level = "debug",
        target = "providers::static_file",
        skip_all
    )]
    pub fn finalize(&mut self) -> ProviderResult<()> {
        if self.prune_on_commit.is_some() {
            return Err(StaticFileWriterError::FinalizeWithPruneQueued.into());
        }
        if self.writer.is_dirty() {
            if !self.synced {
                // Must call self.sync_all() to flush changeset offsets and update
                // the header's changeset_offsets_len, not just the inner writer
                self.sync_all()?;
            }

            self.writer.finalize().map_err(ProviderError::other)?;
            self.update_index()?;
        }
        self.synced = false;
        Ok(())
    }

    /// Commits configuration changes to disk and updates the reader index with the new changes.
    #[instrument(
        name = "StaticFileProviderRW::commit",
        level = "debug",
        target = "providers::static_file",
        skip_all
    )]
    pub fn commit(&mut self) -> ProviderResult<()> {
        let start = Instant::now();

        // Truncates the data file if instructed to.
        if let Some(strategy) = self.prune_on_commit.take() {
            debug!(
                target: "providers::static_file",
                segment = ?self.writer.user_header().segment(),
                "Pruning data on commit"
            );
            match strategy {
                PruneStrategy::Headers { num_blocks } => self.prune_header_data(num_blocks)?,
                PruneStrategy::Transactions { num_rows, last_block } => {
                    self.prune_transaction_data(num_rows, last_block)?
                }
                PruneStrategy::Receipts { num_rows, last_block } => {
                    self.prune_receipt_data(num_rows, last_block)?
                }
                PruneStrategy::TransactionSenders { num_rows, last_block } => {
                    self.prune_transaction_sender_data(num_rows, last_block)?
                }
                PruneStrategy::AccountChangeSets { last_block } => {
                    self.prune_account_changeset_data(last_block)?
                }
                PruneStrategy::StorageChangeSets { last_block } => {
                    self.prune_storage_changeset_data(last_block)?
                }
            }
        }

        // For changeset segments, flush and sync the sidecar file before committing the main file.
        // This ensures crash consistency: the sidecar is durable before the header references it.
        self.flush_current_changeset_offset()?;
        if let Some(writer) = &mut self.changeset_offsets {
            writer.sync().map_err(ProviderError::other)?;
            // Update the header with the actual number of offsets written
            self.writer.user_header_mut().set_changeset_offsets_len(writer.len());
        }

        if self.writer.is_dirty() {
            debug!(
                target: "providers::static_file",
                segment = ?self.writer.user_header().segment(),
                "Committing writer to disk"
            );

            // Commits offsets and new user_header to disk
            self.writer.commit().map_err(ProviderError::other)?;

            if let Some(metrics) = &self.metrics {
                metrics.record_segment_operation(
                    self.writer.user_header().segment(),
                    StaticFileProviderOperation::CommitWriter,
                    Some(start.elapsed()),
                );
            }

            debug!(
                target: "providers::static_file",
                segment = ?self.writer.user_header().segment(),
                path = ?self.data_path,
                duration = ?start.elapsed(),
                "Committed writer to disk"
            );

            self.update_index()?;
        }

        Ok(())
    }

    /// Commits configuration changes to disk and updates the reader index with the new changes.
    ///
    /// CAUTION: does not call `sync_all` on the files.
    #[cfg(feature = "test-utils")]
    pub fn commit_without_sync_all(&mut self) -> ProviderResult<()> {
        let start = Instant::now();

        debug!(
            target: "providers::static_file",
            segment = ?self.writer.user_header().segment(),
            "Committing writer to disk (without sync)"
        );

        // Commits offsets and new user_header to disk
        self.writer.commit_without_sync_all().map_err(ProviderError::other)?;

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                self.writer.user_header().segment(),
                StaticFileProviderOperation::CommitWriter,
                Some(start.elapsed()),
            );
        }

        debug!(
            target: "providers::static_file",
            segment = ?self.writer.user_header().segment(),
            path = ?self.data_path,
            duration = ?start.elapsed(),
            "Committed writer to disk (without sync)"
        );

        self.update_index()?;

        Ok(())
    }

    /// Updates the `self.reader` internal index.
    fn update_index(&self) -> ProviderResult<()> {
        // We find the maximum block of the segment by checking this writer's last block.
        //
        // However if there's no block range (because there's no data), we try to calculate it by
        // subtracting 1 from the expected block start, resulting on the last block of the
        // previous file.
        //
        // If that expected block start is 0, then it means that there's no actual block data, and
        // there's no block data in static files.
        let segment_max_block = self
            .writer
            .user_header()
            .block_range()
            .as_ref()
            .map(|block_range| block_range.end())
            .or_else(|| {
                (self.writer.user_header().expected_block_start() >
                    self.reader().genesis_block_number())
                .then(|| self.writer.user_header().expected_block_start() - 1)
            });

        self.reader().update_index(self.writer.user_header().segment(), segment_max_block)
    }

    /// Ensures that the writer is positioned at the specified block number.
    ///
    /// If the writer is positioned at a greater block number than the specified one, the writer
    /// will NOT be unwound and the error will be returned.
    pub fn ensure_at_block(&mut self, advance_to: BlockNumber) -> ProviderResult<()> {
        let current_block = if let Some(current_block_number) = self.current_block_number() {
            current_block_number
        } else {
            self.increment_block(0)?;
            0
        };

        match current_block.cmp(&advance_to) {
            Ordering::Less => {
                for block in current_block + 1..=advance_to {
                    self.increment_block(block)?;
                }
            }
            Ordering::Equal => {}
            Ordering::Greater => {
                return Err(ProviderError::UnexpectedStaticFileBlockNumber(
                    self.writer.user_header().segment(),
                    current_block,
                    advance_to,
                ));
            }
        }

        Ok(())
    }

    /// Allows to increment the [`SegmentHeader`] end block. It will commit the current static file,
    /// and create the next one if we are past the end range.
    pub fn increment_block(&mut self, expected_block_number: BlockNumber) -> ProviderResult<()> {
        let segment = self.writer.user_header().segment();

        self.check_next_block_number(expected_block_number)?;

        let start = Instant::now();
        if let Some(last_block) = self.writer.user_header().block_end() {
            // We have finished the previous static file and must freeze it
            if last_block == self.writer.user_header().expected_block_end() {
                // Commits offsets and new user_header to disk
                self.commit()?;

                // Opens the new static file
                let (writer, data_path) =
                    Self::open(segment, last_block + 1, self.reader.clone(), self.metrics.clone())?;
                self.writer = writer;
                self.data_path = data_path.clone();

                // Update changeset offsets writer for the new file (starts empty)
                if segment.is_change_based() {
                    let csoff_path = data_path.with_extension("csoff");
                    self.changeset_offsets = Some(
                        ChangesetOffsetWriter::new(&csoff_path, 0).map_err(ProviderError::other)?,
                    );
                }

                *self.writer.user_header_mut() = SegmentHeader::new(
                    self.reader().find_fixed_range(segment, last_block + 1),
                    None,
                    None,
                    segment,
                );
            }
        }

        self.writer.user_header_mut().increment_block();

        // Handle changeset offset tracking for changeset segments
        if segment.is_change_based() {
            // Write previous block's offset if we have one
            if let Some(offset) = self.current_changeset_offset.take() &&
                let Some(writer) = &mut self.changeset_offsets
            {
                writer.append(&offset).map_err(ProviderError::other)?;
            }
            // Start tracking new block's offset
            let new_offset = self.writer.rows() as u64;
            self.current_changeset_offset = Some(ChangesetOffset::new(new_offset, 0));
        }

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                segment,
                StaticFileProviderOperation::IncrementBlock,
                Some(start.elapsed()),
            );
        }

        Ok(())
    }

    /// Returns the current block number of the static file writer.
    pub fn current_block_number(&self) -> Option<u64> {
        self.writer.user_header().block_end()
    }

    /// Returns a block number that is one next to the current tip of static files.
    pub fn next_block_number(&self) -> u64 {
        // The next static file block number can be found by checking the one after block_end.
        // However, if it's a new file that hasn't been added any data, its block range will
        // actually be None. In that case, the next block will be found on `expected_block_start`.
        self.writer
            .user_header()
            .block_end()
            .map(|b| b + 1)
            .unwrap_or_else(|| self.writer.user_header().expected_block_start())
    }

    /// Verifies if the incoming block number matches the next expected block number
    /// for a static file. This ensures data continuity when adding new blocks.
    fn check_next_block_number(&self, expected_block_number: u64) -> ProviderResult<()> {
        let next_static_file_block = self.next_block_number();

        if expected_block_number != next_static_file_block {
            return Err(ProviderError::UnexpectedStaticFileBlockNumber(
                self.writer.user_header().segment(),
                expected_block_number,
                next_static_file_block,
            ))
        }
        Ok(())
    }

    /// Truncates account changesets to the given block. It deletes and loads an older static file
    /// if the block goes beyond the start of the current block range.
    ///
    /// # Note
    /// Commits to the configuration file at the end
    fn truncate_changesets(&mut self, last_block: u64) -> ProviderResult<()> {
        let segment = self.writer.user_header().segment();
        debug_assert!(segment.is_change_based());

        // Get the current block range
        let current_block_end = self
            .writer
            .user_header()
            .block_end()
            .ok_or(ProviderError::MissingStaticFileBlock(segment, 0))?;

        // If we're already at or before the target block, nothing to do
        if current_block_end <= last_block {
            return Ok(())
        }

        // Navigate to the correct file if the target block is in a previous file
        let mut expected_block_start = self.writer.user_header().expected_block_start();
        while last_block < expected_block_start && expected_block_start > 0 {
            self.delete_current_and_open_previous()?;
            expected_block_start = self.writer.user_header().expected_block_start();
        }

        // Find the number of rows to keep (up to and including last_block)
        let blocks_to_keep = if last_block >= expected_block_start {
            last_block - expected_block_start + 1
        } else {
            0
        };

        // Read changeset offsets from sidecar file to find where to truncate
        let csoff_path = self.data_path.with_extension("csoff");
        let changeset_offsets_len = self.writer.user_header().changeset_offsets_len();

        // Flush any pending changeset offset before reading the sidecar
        self.flush_current_changeset_offset()?;

        let rows_to_keep = if blocks_to_keep == 0 {
            0
        } else if blocks_to_keep >= changeset_offsets_len {
            // Keep all rows in this file
            self.writer.rows() as u64
        } else {
            // Read offset for the block after last_block from sidecar.
            // Use committed length from header, ignoring any uncommitted records
            // that may exist in the file after a crash.
            let mut reader = ChangesetOffsetReader::new(&csoff_path, changeset_offsets_len)
                .map_err(ProviderError::other)?;
            if let Some(next_offset) = reader.get(blocks_to_keep).map_err(ProviderError::other)? {
                next_offset.offset()
            } else {
                // If we can't read the offset, keep all rows
                self.writer.rows() as u64
            }
        };

        let total_rows = self.writer.rows() as u64;
        let rows_to_delete = total_rows.saturating_sub(rows_to_keep);

        if rows_to_delete > 0 {
            // Calculate the number of blocks to prune
            let current_block_end = self
                .writer
                .user_header()
                .block_end()
                .ok_or(ProviderError::MissingStaticFileBlock(segment, 0))?;
            let blocks_to_remove = current_block_end - last_block;

            // Update segment header - for changesets, prune expects number of blocks, not rows
            self.writer.user_header_mut().prune(blocks_to_remove);

            // Prune the actual rows
            self.writer.prune_rows(rows_to_delete as usize).map_err(ProviderError::other)?;
        }

        // Update the block range
        self.writer.user_header_mut().set_block_range(expected_block_start, last_block);

        // Sync changeset offsets to match the new block range
        self.writer.user_header_mut().sync_changeset_offsets();

        // Truncate the sidecar file to match the new block count
        if let Some(writer) = &mut self.changeset_offsets {
            writer.truncate(blocks_to_keep).map_err(ProviderError::other)?;
        }

        // Clear current changeset offset tracking since we've pruned
        self.current_changeset_offset = None;

        // Commits new changes to disk
        self.commit()?;

        Ok(())
    }

    /// Truncates a number of rows from disk. It deletes and loads an older static file if block
    /// goes beyond the start of the current block range.
    ///
    /// **`last_block`** should be passed only with transaction based segments.
    ///
    /// # Note
    /// Commits to the configuration file at the end.
    fn truncate(&mut self, num_rows: u64, last_block: Option<u64>) -> ProviderResult<()> {
        let mut remaining_rows = num_rows;
        let segment = self.writer.user_header().segment();
        while remaining_rows > 0 {
            let len = if segment.is_block_based() {
                self.writer.user_header().block_len().unwrap_or_default()
            } else {
                self.writer.user_header().tx_len().unwrap_or_default()
            };

            if remaining_rows >= len {
                // If there's more rows to delete than this static file contains, then just
                // delete the whole file and go to the next static file
                let block_start = self.writer.user_header().expected_block_start();

                // We only delete the file if it's NOT the first static file AND:
                // * it's a Header segment  OR
                // * it's a tx-based segment AND `last_block` is lower than the first block of this
                //   file's block range. Otherwise, having no rows simply means that this block
                //   range has no transactions, but the file should remain.
                if block_start != 0 &&
                    (segment.is_headers() || last_block.is_some_and(|b| b < block_start))
                {
                    self.delete_current_and_open_previous()?;
                } else {
                    // Update `SegmentHeader`
                    self.writer.user_header_mut().prune(len);
                    self.writer.prune_rows(len as usize).map_err(ProviderError::other)?;
                    break
                }

                remaining_rows -= len;
            } else {
                // Update `SegmentHeader`
                self.writer.user_header_mut().prune(remaining_rows);

                // Truncate data
                self.writer.prune_rows(remaining_rows as usize).map_err(ProviderError::other)?;
                remaining_rows = 0;
            }
        }

        // Only Transactions and Receipts
        if let Some(last_block) = last_block {
            let mut expected_block_start = self.writer.user_header().expected_block_start();

            if num_rows == 0 {
                // Edge case for when we are unwinding a chain of empty blocks that goes across
                // files, and therefore, the only reference point to know which file
                // we are supposed to be at is `last_block`.
                while last_block < expected_block_start {
                    self.delete_current_and_open_previous()?;
                    expected_block_start = self.writer.user_header().expected_block_start();
                }
            }
            self.writer.user_header_mut().set_block_range(expected_block_start, last_block);
        }

        // Commits new changes to disk.
        self.commit()?;

        Ok(())
    }

    /// Delete the current static file, and replace this provider writer with the previous static
    /// file.
    fn delete_current_and_open_previous(&mut self) -> Result<(), ProviderError> {
        let segment = self.user_header().segment();
        let current_path = self.data_path.clone();
        let (previous_writer, data_path) = Self::open(
            segment,
            self.writer.user_header().expected_block_start() - 1,
            self.reader.clone(),
            self.metrics.clone(),
        )?;
        self.writer = previous_writer;
        self.writer.set_dirty();
        self.data_path = data_path.clone();

        // Delete the sidecar file for changeset segments before deleting the main jar
        if segment.is_change_based() {
            let csoff_path = current_path.with_extension("csoff");
            if csoff_path.exists() {
                std::fs::remove_file(&csoff_path).map_err(ProviderError::other)?;
            }
            // Re-initialize the changeset offsets writer for the previous file
            let new_csoff_path = data_path.with_extension("csoff");
            let committed_len = self.writer.user_header().changeset_offsets_len();
            self.changeset_offsets = Some(
                ChangesetOffsetWriter::new(&new_csoff_path, committed_len)
                    .map_err(ProviderError::other)?,
            );
        }

        // Clear current changeset offset tracking since we're switching files
        self.current_changeset_offset = None;

        NippyJar::<SegmentHeader>::load(&current_path)
            .map_err(ProviderError::other)?
            .delete()
            .map_err(ProviderError::other)?;
        Ok(())
    }

    /// Appends column to static file.
    fn append_column<T: Compact>(&mut self, column: T) -> ProviderResult<()> {
        self.buf.clear();
        column.to_compact(&mut self.buf);

        self.writer.append_column(Some(Ok(&self.buf))).map_err(ProviderError::other)?;
        Ok(())
    }

    /// Appends to tx number-based static file.
    fn append_with_tx_number<V: Compact>(
        &mut self,
        tx_num: TxNumber,
        value: V,
    ) -> ProviderResult<()> {
        if let Some(range) = self.writer.user_header().tx_range() {
            let next_tx = range.end() + 1;
            if next_tx != tx_num {
                return Err(ProviderError::UnexpectedStaticFileTxNumber(
                    self.writer.user_header().segment(),
                    tx_num,
                    next_tx,
                ))
            }
            self.writer.user_header_mut().increment_tx();
        } else {
            self.writer.user_header_mut().set_tx_range(tx_num, tx_num);
        }

        self.append_column(value)?;

        Ok(())
    }

    /// Appends change to changeset static file.
    fn append_change<V: Compact>(&mut self, change: &V) -> ProviderResult<()> {
        if let Some(ref mut offset) = self.current_changeset_offset {
            offset.increment_num_changes();
        }
        self.append_column(change)?;
        Ok(())
    }

    /// Appends header to static file.
    ///
    /// It **CALLS** `increment_block()` since the number of headers is equal to the number of
    /// blocks.
    pub fn append_header(&mut self, header: &N::BlockHeader, hash: &BlockHash) -> ProviderResult<()>
    where
        N::BlockHeader: Compact,
    {
        self.append_header_with_td(header, U256::ZERO, hash)
    }

    /// Appends header to static file with a specified total difficulty.
    ///
    /// It **CALLS** `increment_block()` since the number of headers is equal to the number of
    /// blocks.
    pub fn append_header_with_td(
        &mut self,
        header: &N::BlockHeader,
        total_difficulty: U256,
        hash: &BlockHash,
    ) -> ProviderResult<()>
    where
        N::BlockHeader: Compact,
    {
        let start = Instant::now();
        self.ensure_no_queued_prune()?;

        debug_assert!(self.writer.user_header().segment() == StaticFileSegment::Headers);

        self.increment_block(header.number())?;

        self.append_column(header)?;
        self.append_column(CompactU256::from(total_difficulty))?;
        self.append_column(hash)?;

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                StaticFileSegment::Headers,
                StaticFileProviderOperation::Append,
                Some(start.elapsed()),
            );
        }

        Ok(())
    }

    /// Appends header to static file without calling `increment_block`.
    /// This is useful for genesis blocks with non-zero block numbers.
    pub fn append_header_direct(
        &mut self,
        header: &N::BlockHeader,
        total_difficulty: U256,
        hash: &BlockHash,
    ) -> ProviderResult<()>
    where
        N::BlockHeader: Compact,
    {
        let start = Instant::now();
        self.ensure_no_queued_prune()?;

        debug_assert!(self.writer.user_header().segment() == StaticFileSegment::Headers);

        self.append_column(header)?;
        self.append_column(CompactU256::from(total_difficulty))?;
        self.append_column(hash)?;

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                StaticFileSegment::Headers,
                StaticFileProviderOperation::Append,
                Some(start.elapsed()),
            );
        }

        Ok(())
    }

    /// Appends transaction to static file.
    ///
    /// It **DOES NOT CALL** `increment_block()`, it should be handled elsewhere. There might be
    /// empty blocks and this function wouldn't be called.
    pub fn append_transaction(&mut self, tx_num: TxNumber, tx: &N::SignedTx) -> ProviderResult<()>
    where
        N::SignedTx: Compact,
    {
        let start = Instant::now();
        self.ensure_no_queued_prune()?;

        debug_assert!(self.writer.user_header().segment() == StaticFileSegment::Transactions);
        self.append_with_tx_number(tx_num, tx)?;

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                StaticFileSegment::Transactions,
                StaticFileProviderOperation::Append,
                Some(start.elapsed()),
            );
        }

        Ok(())
    }

    /// Appends receipt to static file.
    ///
    /// It **DOES NOT** call `increment_block()`, it should be handled elsewhere. There might be
    /// empty blocks and this function wouldn't be called.
    pub fn append_receipt(&mut self, tx_num: TxNumber, receipt: &N::Receipt) -> ProviderResult<()>
    where
        N::Receipt: Compact,
    {
        let start = Instant::now();
        self.ensure_no_queued_prune()?;

        debug_assert!(self.writer.user_header().segment() == StaticFileSegment::Receipts);
        self.append_with_tx_number(tx_num, receipt)?;

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                StaticFileSegment::Receipts,
                StaticFileProviderOperation::Append,
                Some(start.elapsed()),
            );
        }

        Ok(())
    }

    /// Appends multiple receipts to the static file.
    pub fn append_receipts<I, R>(&mut self, receipts: I) -> ProviderResult<()>
    where
        I: Iterator<Item = Result<(TxNumber, R), ProviderError>>,
        R: Borrow<N::Receipt>,
        N::Receipt: Compact,
    {
        debug_assert!(self.writer.user_header().segment() == StaticFileSegment::Receipts);

        let mut receipts_iter = receipts.into_iter().peekable();
        // If receipts are empty, we can simply return None
        if receipts_iter.peek().is_none() {
            return Ok(());
        }

        let start = Instant::now();
        self.ensure_no_queued_prune()?;

        // At this point receipts contains at least one receipt, so this would be overwritten.
        let mut count: u64 = 0;

        for receipt_result in receipts_iter {
            let (tx_num, receipt) = receipt_result?;
            self.append_with_tx_number(tx_num, receipt.borrow())?;
            count += 1;
        }

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operations(
                StaticFileSegment::Receipts,
                StaticFileProviderOperation::Append,
                count,
                Some(start.elapsed()),
            );
        }

        Ok(())
    }

    /// Appends transaction sender to static file.
    ///
    /// It **DOES NOT** call `increment_block()`, it should be handled elsewhere. There might be
    /// empty blocks and this function wouldn't be called.
    pub fn append_transaction_sender(
        &mut self,
        tx_num: TxNumber,
        sender: &alloy_primitives::Address,
    ) -> ProviderResult<()> {
        let start = Instant::now();
        self.ensure_no_queued_prune()?;

        debug_assert!(self.writer.user_header().segment() == StaticFileSegment::TransactionSenders);
        self.append_with_tx_number(tx_num, sender)?;

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                StaticFileSegment::TransactionSenders,
                StaticFileProviderOperation::Append,
                Some(start.elapsed()),
            );
        }

        Ok(())
    }

    /// Appends multiple transaction senders to the static file.
    pub fn append_transaction_senders<I>(&mut self, senders: I) -> ProviderResult<()>
    where
        I: Iterator<Item = (TxNumber, alloy_primitives::Address)>,
    {
        debug_assert!(self.writer.user_header().segment() == StaticFileSegment::TransactionSenders);

        let mut senders_iter = senders.into_iter().peekable();
        // If senders are empty, we can simply return
        if senders_iter.peek().is_none() {
            return Ok(());
        }

        let start = Instant::now();
        self.ensure_no_queued_prune()?;

        // At this point senders contains at least one sender, so this would be overwritten.
        let mut count: u64 = 0;
        for (tx_num, sender) in senders_iter {
            self.append_with_tx_number(tx_num, sender)?;
            count += 1;
        }

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operations(
                StaticFileSegment::TransactionSenders,
                StaticFileProviderOperation::Append,
                count,
                Some(start.elapsed()),
            );
        }

        Ok(())
    }

    /// Appends a block changeset to the static file.
    ///
    /// It **CALLS** `increment_block()`.
    ///
    /// Returns the current number of changesets in the file, if any.
    pub fn append_account_changeset(
        &mut self,
        mut changeset: Vec<AccountBeforeTx>,
        block_number: u64,
    ) -> ProviderResult<()> {
        debug_assert!(self.writer.user_header().segment() == StaticFileSegment::AccountChangeSets);
        let start = Instant::now();

        self.increment_block(block_number)?;
        self.ensure_no_queued_prune()?;

        // first sort the changeset by address
        changeset.sort_by_key(|change| change.address);

        let mut count: u64 = 0;

        for change in changeset {
            self.append_change(&change)?;
            count += 1;
        }

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operations(
                StaticFileSegment::AccountChangeSets,
                StaticFileProviderOperation::Append,
                count,
                Some(start.elapsed()),
            );
        }

        Ok(())
    }

    /// Appends a block storage changeset to the static file.
    ///
    /// It **CALLS** `increment_block()`.
    pub fn append_storage_changeset(
        &mut self,
        mut changeset: Vec<StorageBeforeTx>,
        block_number: u64,
    ) -> ProviderResult<()> {
        debug_assert!(self.writer.user_header().segment() == StaticFileSegment::StorageChangeSets);
        let start = Instant::now();

        self.increment_block(block_number)?;
        self.ensure_no_queued_prune()?;

        // sort by address + storage key
        changeset.sort_by_key(|change| (change.address, change.key));

        let mut count: u64 = 0;
        for change in changeset {
            self.append_change(&change)?;
            count += 1;
        }

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operations(
                StaticFileSegment::StorageChangeSets,
                StaticFileProviderOperation::Append,
                count,
                Some(start.elapsed()),
            );
        }

        Ok(())
    }

    /// Adds an instruction to prune `to_delete` transactions during commit.
    ///
    /// Note: `last_block` refers to the block the unwinds ends at.
    pub fn prune_transactions(
        &mut self,
        to_delete: u64,
        last_block: BlockNumber,
    ) -> ProviderResult<()> {
        debug_assert_eq!(self.writer.user_header().segment(), StaticFileSegment::Transactions);
        self.queue_prune(PruneStrategy::Transactions { num_rows: to_delete, last_block })
    }

    /// Adds an instruction to prune `to_delete` receipts during commit.
    ///
    /// Note: `last_block` refers to the block the unwinds ends at.
    pub fn prune_receipts(
        &mut self,
        to_delete: u64,
        last_block: BlockNumber,
    ) -> ProviderResult<()> {
        debug_assert_eq!(self.writer.user_header().segment(), StaticFileSegment::Receipts);
        self.queue_prune(PruneStrategy::Receipts { num_rows: to_delete, last_block })
    }

    /// Adds an instruction to prune `to_delete` transaction senders during commit.
    ///
    /// Note: `last_block` refers to the block the unwinds ends at.
    pub fn prune_transaction_senders(
        &mut self,
        to_delete: u64,
        last_block: BlockNumber,
    ) -> ProviderResult<()> {
        debug_assert_eq!(
            self.writer.user_header().segment(),
            StaticFileSegment::TransactionSenders
        );
        self.queue_prune(PruneStrategy::TransactionSenders { num_rows: to_delete, last_block })
    }

    /// Adds an instruction to prune `to_delete` headers during commit.
    pub fn prune_headers(&mut self, to_delete: u64) -> ProviderResult<()> {
        debug_assert_eq!(self.writer.user_header().segment(), StaticFileSegment::Headers);
        self.queue_prune(PruneStrategy::Headers { num_blocks: to_delete })
    }

    /// Adds an instruction to prune changesets until the given block.
    pub fn prune_account_changesets(&mut self, last_block: u64) -> ProviderResult<()> {
        debug_assert_eq!(self.writer.user_header().segment(), StaticFileSegment::AccountChangeSets);
        self.queue_prune(PruneStrategy::AccountChangeSets { last_block })
    }

    /// Adds an instruction to prune storage changesets until the given block.
    pub fn prune_storage_changesets(&mut self, last_block: u64) -> ProviderResult<()> {
        debug_assert_eq!(self.writer.user_header().segment(), StaticFileSegment::StorageChangeSets);
        self.queue_prune(PruneStrategy::StorageChangeSets { last_block })
    }

    /// Adds an instruction to prune elements during commit using the specified strategy.
    fn queue_prune(&mut self, strategy: PruneStrategy) -> ProviderResult<()> {
        self.ensure_no_queued_prune()?;
        self.prune_on_commit = Some(strategy);
        Ok(())
    }

    /// Returns Error if there is a pruning instruction that needs to be applied.
    fn ensure_no_queued_prune(&self) -> ProviderResult<()> {
        if self.prune_on_commit.is_some() {
            return Err(ProviderError::other(StaticFileWriterError::new(
                "Pruning should be committed before appending or pruning more data",
            )));
        }
        Ok(())
    }

    /// Removes the last `to_delete` transactions from the data file.
    fn prune_transaction_data(
        &mut self,
        to_delete: u64,
        last_block: BlockNumber,
    ) -> ProviderResult<()> {
        let start = Instant::now();

        debug_assert!(self.writer.user_header().segment() == StaticFileSegment::Transactions);

        self.truncate(to_delete, Some(last_block))?;

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                StaticFileSegment::Transactions,
                StaticFileProviderOperation::Prune,
                Some(start.elapsed()),
            );
        }

        Ok(())
    }

    /// Prunes the last `to_delete` account changesets from the data file.
    fn prune_account_changeset_data(&mut self, last_block: BlockNumber) -> ProviderResult<()> {
        let start = Instant::now();

        debug_assert!(self.writer.user_header().segment() == StaticFileSegment::AccountChangeSets);

        self.truncate_changesets(last_block)?;

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                StaticFileSegment::AccountChangeSets,
                StaticFileProviderOperation::Prune,
                Some(start.elapsed()),
            );
        }

        Ok(())
    }

    /// Prunes the last storage changesets from the data file.
    fn prune_storage_changeset_data(&mut self, last_block: BlockNumber) -> ProviderResult<()> {
        let start = Instant::now();

        debug_assert!(self.writer.user_header().segment() == StaticFileSegment::StorageChangeSets);

        self.truncate_changesets(last_block)?;

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                StaticFileSegment::StorageChangeSets,
                StaticFileProviderOperation::Prune,
                Some(start.elapsed()),
            );
        }

        Ok(())
    }

    /// Prunes the last `to_delete` receipts from the data file.
    fn prune_receipt_data(
        &mut self,
        to_delete: u64,
        last_block: BlockNumber,
    ) -> ProviderResult<()> {
        let start = Instant::now();

        debug_assert!(self.writer.user_header().segment() == StaticFileSegment::Receipts);

        self.truncate(to_delete, Some(last_block))?;

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                StaticFileSegment::Receipts,
                StaticFileProviderOperation::Prune,
                Some(start.elapsed()),
            );
        }

        Ok(())
    }

    /// Prunes the last `to_delete` transaction senders from the data file.
    fn prune_transaction_sender_data(
        &mut self,
        to_delete: u64,
        last_block: BlockNumber,
    ) -> ProviderResult<()> {
        let start = Instant::now();

        debug_assert!(self.writer.user_header().segment() == StaticFileSegment::TransactionSenders);

        self.truncate(to_delete, Some(last_block))?;

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                StaticFileSegment::TransactionSenders,
                StaticFileProviderOperation::Prune,
                Some(start.elapsed()),
            );
        }

        Ok(())
    }

    /// Prunes the last `to_delete` headers from the data file.
    fn prune_header_data(&mut self, to_delete: u64) -> ProviderResult<()> {
        let start = Instant::now();

        debug_assert!(self.writer.user_header().segment() == StaticFileSegment::Headers);

        self.truncate(to_delete, None)?;

        if let Some(metrics) = &self.metrics {
            metrics.record_segment_operation(
                StaticFileSegment::Headers,
                StaticFileProviderOperation::Prune,
                Some(start.elapsed()),
            );
        }

        Ok(())
    }

    /// Returns a [`StaticFileProvider`] associated with this writer.
    pub fn reader(&self) -> StaticFileProvider<N> {
        Self::upgrade_provider_to_strong_reference(&self.reader)
    }

    /// Upgrades a weak reference of [`StaticFileProviderInner`] to a strong reference
    /// [`StaticFileProvider`].
    ///
    /// # Panics
    ///
    /// Panics if the parent [`StaticFileProvider`] is fully dropped while the child writer is still
    /// active. In reality, it's impossible to detach the [`StaticFileProviderRW`] from the
    /// [`StaticFileProvider`].
    fn upgrade_provider_to_strong_reference(
        provider: &Weak<StaticFileProviderInner<N>>,
    ) -> StaticFileProvider<N> {
        provider.upgrade().map(StaticFileProvider).expect("StaticFileProvider is dropped")
    }

    /// Helper function to access [`SegmentHeader`].
    pub const fn user_header(&self) -> &SegmentHeader {
        self.writer.user_header()
    }

    /// Helper function to access a mutable reference to [`SegmentHeader`].
    pub const fn user_header_mut(&mut self) -> &mut SegmentHeader {
        self.writer.user_header_mut()
    }

    /// Helper function to override block range for testing.
    #[cfg(any(test, feature = "test-utils"))]
    pub const fn set_block_range(&mut self, block_range: std::ops::RangeInclusive<BlockNumber>) {
        self.writer.user_header_mut().set_block_range(*block_range.start(), *block_range.end())
    }

    /// Helper function to override block range for testing.
    #[cfg(any(test, feature = "test-utils"))]
    pub const fn inner(&mut self) -> &mut NippyJarWriter<SegmentHeader> {
        &mut self.writer
    }
}

fn create_jar(
    segment: StaticFileSegment,
    path: &Path,
    expected_block_range: SegmentRangeInclusive,
) -> NippyJar<SegmentHeader> {
    let mut jar = NippyJar::new(
        segment.columns(),
        path,
        SegmentHeader::new(expected_block_range, None, None, segment),
    );

    // Transaction and Receipt already have the compression scheme used natively in its encoding.
    // (zstd-dictionary)
    if segment.is_headers() {
        jar = jar.with_lz4();
    }

    jar
}

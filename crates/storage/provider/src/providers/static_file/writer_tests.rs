//! Crash recovery tests for changeset offset healing.
//!
//! These tests verify the three-way consistency healing logic between:
//! 1. Header (`SegmentHeader.changeset_offsets_len`)
//! 2. `NippyJar` rows (actual row count)
//! 3. Sidecar file (.csoff)
//!
//! All tests use real `NippyJar` files via `StaticFileProvider` to test the full healing path.

#[cfg(test)]
mod tests {
    use alloy_primitives::{Address, U256};
    use reth_db::{models::AccountBeforeTx, test_utils::create_test_static_files_dir};
    use reth_primitives_traits::Account;
    use reth_static_file_types::{ChangesetOffset, ChangesetOffsetReader, StaticFileSegment};
    use std::{fs::OpenOptions, io::Write as _, path::PathBuf};

    use crate::providers::{
        static_file::manager::{StaticFileProviderBuilder, StaticFileWriter},
        StaticFileProvider,
    };
    use reth_chain_state::EthPrimitives;

    // ==================== HELPER FUNCTIONS ====================

    /// Creates a `StaticFileProvider` for testing with the given `blocks_per_file` setting.
    fn setup_test_provider(
        static_dir: &tempfile::TempDir,
        blocks_per_file: u64,
    ) -> StaticFileProvider<EthPrimitives> {
        StaticFileProviderBuilder::read_write(static_dir)
            .with_blocks_per_file(blocks_per_file)
            .build()
            .expect("Failed to build static file provider")
    }

    /// Generates test changeset data for a block.
    fn generate_test_changeset(block_num: u64, num_changes: usize) -> Vec<AccountBeforeTx> {
        (0..num_changes)
            .map(|i| {
                let mut address = Address::ZERO;
                address.0[0] = block_num as u8;
                address.0[1] = i as u8;
                AccountBeforeTx {
                    address,
                    info: Some(Account {
                        nonce: block_num,
                        balance: U256::from(block_num * 1000 + i as u64),
                        bytecode_hash: None,
                    }),
                }
            })
            .collect()
    }

    /// Writes test blocks to the `AccountChangeSets` segment.
    /// Returns the path to the sidecar file.
    fn write_test_blocks(
        provider: &StaticFileProvider<EthPrimitives>,
        num_blocks: u64,
        changes_per_block: usize,
    ) -> PathBuf {
        let mut writer = provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();

        for block_num in 0..num_blocks {
            let changeset = generate_test_changeset(block_num, changes_per_block);
            writer.append_account_changeset(changeset, block_num).unwrap();
        }

        writer.commit().unwrap();

        // Return path to sidecar
        get_sidecar_path(provider, 0)
    }

    /// Gets the .csoff sidecar path for a given block.
    fn get_sidecar_path(provider: &StaticFileProvider<EthPrimitives>, block: u64) -> PathBuf {
        let range = provider.find_fixed_range(StaticFileSegment::AccountChangeSets, block);
        let filename = StaticFileSegment::AccountChangeSets.filename(&range);
        provider.directory().join(filename).with_extension("csoff")
    }

    /// Reads the block count from a sidecar file (file size / 16).
    fn get_sidecar_block_count(path: &PathBuf) -> u64 {
        if !path.exists() {
            return 0;
        }
        let metadata = std::fs::metadata(path).unwrap();
        metadata.len() / 16
    }

    /// Appends a partial record to sidecar (simulates crash mid-write).
    fn corrupt_sidecar_partial_write(path: &PathBuf, partial_bytes: usize) {
        let mut file = OpenOptions::new().append(true).open(path).unwrap();
        file.write_all(&vec![0u8; partial_bytes]).unwrap();
        file.sync_all().unwrap();
    }

    /// Appends fake blocks to sidecar that point past actual `NippyJar` rows.
    fn corrupt_sidecar_add_fake_blocks(path: &PathBuf, num_fake_blocks: u64, start_offset: u64) {
        let mut file = OpenOptions::new().append(true).open(path).unwrap();
        for i in 0..num_fake_blocks {
            let offset = ChangesetOffset::new(start_offset + i * 5, 5);
            let mut buf = [0u8; 16];
            buf[..8].copy_from_slice(&offset.offset().to_le_bytes());
            buf[8..].copy_from_slice(&offset.num_changes().to_le_bytes());
            file.write_all(&buf).unwrap();
        }
        file.sync_all().unwrap();
    }

    /// Truncates the sidecar file to a specific number of blocks.
    fn truncate_sidecar(path: &PathBuf, num_blocks: u64) {
        let file = OpenOptions::new().write(true).open(path).unwrap();
        file.set_len(num_blocks * 16).unwrap();
        file.sync_all().unwrap();
    }

    /// Reads the `changeset_offsets_len` from the segment header.
    fn get_header_block_count(provider: &StaticFileProvider<EthPrimitives>, block: u64) -> u64 {
        let jar_provider = provider
            .get_segment_provider_for_block(StaticFileSegment::AccountChangeSets, block, None)
            .unwrap();
        jar_provider.user_header().changeset_offsets_len()
    }

    /// Gets the actual row count from `NippyJar`.
    fn get_nippy_row_count(provider: &StaticFileProvider<EthPrimitives>, block: u64) -> u64 {
        let jar_provider = provider
            .get_segment_provider_for_block(StaticFileSegment::AccountChangeSets, block, None)
            .unwrap();
        jar_provider.rows() as u64
    }

    // ==================== APPEND CRASH SCENARIOS ====================

    #[test]
    fn test_append_crash_partial_sidecar_record() {
        // SCENARIO: Crash mid-write of a 16-byte sidecar record.
        // State after crash: Sidecar has N complete records + partial bytes.
        // Expected: Healing truncates partial bytes, keeps N complete records.

        let (static_dir, _) = create_test_static_files_dir();
        let provider = setup_test_provider(&static_dir, 100);

        // Write 5 blocks with 3 changes each
        let sidecar_path = write_test_blocks(&provider, 5, 3);
        assert_eq!(get_sidecar_block_count(&sidecar_path), 5);

        // Corrupt: append partial record (8 of 16 bytes)
        corrupt_sidecar_partial_write(&sidecar_path, 8);
        assert_eq!(
            std::fs::metadata(&sidecar_path).unwrap().len(),
            5 * 16 + 8,
            "Should have 5 records + 8 partial bytes"
        );

        // Reopen provider - triggers healing
        drop(provider);
        let provider = setup_test_provider(&static_dir, 100);

        // Verify healing truncated partial record
        let _writer = provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();
        assert_eq!(get_sidecar_block_count(&sidecar_path), 5, "Should have 5 complete records");
        assert_eq!(
            std::fs::metadata(&sidecar_path).unwrap().len(),
            5 * 16,
            "File should be exactly 80 bytes"
        );
    }

    #[test]
    fn test_append_crash_sidecar_synced_header_not_committed() {
        // SCENARIO: Sidecar was synced with new blocks, but header commit crashed.
        // State after crash: Sidecar has more blocks than header claims.
        // Expected: Healing clamps to header value (never enlarges header).

        let (static_dir, _) = create_test_static_files_dir();
        let provider = setup_test_provider(&static_dir, 100);

        // Write 5 blocks
        let sidecar_path = write_test_blocks(&provider, 5, 3);
        let total_rows = 5 * 3; // 15 rows

        // Corrupt: add 3 fake blocks to sidecar (simulates sidecar sync before header commit)
        // These blocks point to valid rows (within the 15 rows we have)
        // But header doesn't know about them
        corrupt_sidecar_add_fake_blocks(&sidecar_path, 3, total_rows as u64);
        assert_eq!(get_sidecar_block_count(&sidecar_path), 8, "Sidecar should have 8 blocks");

        // Reopen - healing should clamp to header's 5 blocks
        drop(provider);
        let provider = setup_test_provider(&static_dir, 100);
        let _writer = provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();

        // Header is authoritative - sidecar should be truncated to 5
        assert_eq!(get_sidecar_block_count(&sidecar_path), 5, "Should clamp to header value");
        assert_eq!(get_header_block_count(&provider, 0), 5);
    }

    #[test]
    fn test_append_crash_sidecar_ahead_of_nippy_offsets() {
        // APPEND CP2: After data sync, before offsets sync.
        // SCENARIO: Sidecar was synced first (has new blocks), data file has new rows,
        // but NippyJar offsets file is stale. Config is stale.
        //
        // We can't easily simulate NippyJar internal offset mismatch, but we CAN test
        // that sidecar entries are validated against actual_nippy_rows (from NippyJar.rows()).
        // If sidecar claims blocks that would exceed NippyJar's row count, healing truncates.

        let (static_dir, _) = create_test_static_files_dir();
        let provider = setup_test_provider(&static_dir, 100);

        // Write 5 blocks with 3 changes each (15 rows total)
        let sidecar_path = write_test_blocks(&provider, 5, 3);
        assert_eq!(get_nippy_row_count(&provider, 0), 15);
        assert_eq!(get_sidecar_block_count(&sidecar_path), 5);

        // Corrupt sidecar: add 3 fake blocks that claim to point to rows beyond NippyJar's 15
        // Block 6: offset=15, count=3 (rows 15-17, but only 15 rows exist!)
        // Block 7: offset=18, count=3 (rows 18-20, invalid)
        // Block 8: offset=21, count=3 (rows 21-23, invalid)
        corrupt_sidecar_add_fake_blocks(&sidecar_path, 3, 15);
        assert_eq!(get_sidecar_block_count(&sidecar_path), 8);

        // Reopen - healing should detect sidecar offsets point past actual NippyJar rows
        // and truncate back. Since header is 5, healing clamps to min(8, 5) = 5.
        drop(provider);
        let provider = setup_test_provider(&static_dir, 100);
        let _writer = provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();

        // Sidecar should be clamped to header's 5 blocks
        assert_eq!(get_sidecar_block_count(&sidecar_path), 5);
        assert_eq!(get_header_block_count(&provider, 0), 5);
        // NippyJar rows unchanged
        assert_eq!(get_nippy_row_count(&provider, 0), 15);
    }

    #[test]
    fn test_append_clean_no_crash() {
        // BASELINE: Normal append with no crash.
        // All three sources should be in sync.

        let (static_dir, _) = create_test_static_files_dir();
        let provider = setup_test_provider(&static_dir, 100);

        // Write 10 blocks with 5 changes each
        let sidecar_path = write_test_blocks(&provider, 10, 5);

        // Verify all in sync
        assert_eq!(get_sidecar_block_count(&sidecar_path), 10);
        assert_eq!(get_header_block_count(&provider, 0), 10);
        assert_eq!(get_nippy_row_count(&provider, 0), 50); // 10 blocks * 5 changes

        // Reopen multiple times - should remain stable
        drop(provider);
        for _ in 0..3 {
            let provider = setup_test_provider(&static_dir, 100);
            let _writer = provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();
            assert_eq!(get_sidecar_block_count(&sidecar_path), 10);
        }
    }

    // ==================== PRUNE CRASH SCENARIOS ====================

    #[test]
    fn test_prune_crash_sidecar_truncated_header_stale() {
        // SCENARIO: Prune truncated sidecar but crashed before header commit.
        // State after crash: Sidecar has fewer blocks than header claims.
        // Expected: Healing updates header to match sidecar.

        let (static_dir, _) = create_test_static_files_dir();
        let provider = setup_test_provider(&static_dir, 100);

        // Write 10 blocks
        let sidecar_path = write_test_blocks(&provider, 10, 5);
        assert_eq!(get_sidecar_block_count(&sidecar_path), 10);

        // Simulate prune crash: truncate sidecar to 7 blocks but don't update header
        // (In real crash, header would still claim 10 blocks)
        truncate_sidecar(&sidecar_path, 7);
        assert_eq!(get_sidecar_block_count(&sidecar_path), 7);

        // Reopen - healing should detect sidecar < header and fix header
        drop(provider);
        let provider = setup_test_provider(&static_dir, 100);
        {
            let _writer = provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();
            // Writer commits healed header on open
        }
        // Drop writer and reopen provider to see committed changes
        drop(provider);
        let provider = setup_test_provider(&static_dir, 100);

        // After healing, header should match sidecar
        assert_eq!(get_sidecar_block_count(&sidecar_path), 7);
        assert_eq!(get_header_block_count(&provider, 0), 7);
    }

    #[test]
    fn test_prune_crash_sidecar_offsets_past_nippy_rows() {
        // SCENARIO: NippyJar was pruned but sidecar wasn't truncated.
        // State after crash: Sidecar has offsets pointing past actual NippyJar rows.
        // Expected: Healing validates offsets and truncates invalid blocks.

        let (static_dir, _) = create_test_static_files_dir();
        let provider = setup_test_provider(&static_dir, 100);

        // Write 10 blocks with 5 changes each (50 rows total)
        let sidecar_path = write_test_blocks(&provider, 10, 5);

        // Verify initial state
        assert_eq!(get_nippy_row_count(&provider, 0), 50);
        assert_eq!(get_sidecar_block_count(&sidecar_path), 10);

        // Now add fake blocks to sidecar that point past row 50
        // These simulate a crash where sidecar wasn't cleaned up after NippyJar prune
        corrupt_sidecar_add_fake_blocks(&sidecar_path, 5, 50); // Blocks pointing to rows 50-74
        assert_eq!(get_sidecar_block_count(&sidecar_path), 15);

        // Reopen - healing should detect invalid offsets and truncate
        drop(provider);
        let provider = setup_test_provider(&static_dir, 100);
        let _writer = provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();

        // Should be back to 10 valid blocks (offsets pointing within 50 rows)
        assert_eq!(get_sidecar_block_count(&sidecar_path), 10);
        assert_eq!(get_header_block_count(&provider, 0), 10);
    }

    #[test]
    fn test_prune_crash_nippy_offsets_truncated_data_stale() {
        // PRUNE CP1-3: Tests healing when sidecar has offsets past NippyJar.rows().
        // NippyJar heals internally, then changeset healing validates against rows().
        // Verifies healing uses actual_nippy_rows as source of truth.

        let (static_dir, _) = create_test_static_files_dir();
        let provider = setup_test_provider(&static_dir, 100);

        // Write 10 blocks with varying changes
        {
            let mut writer = provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();
            for block_num in 0..10 {
                let changes = (block_num % 5 + 1) as usize; // 1-5 changes per block
                writer
                    .append_account_changeset(
                        generate_test_changeset(block_num, changes),
                        block_num,
                    )
                    .unwrap();
            }
            writer.commit().unwrap();
        }

        let sidecar_path = get_sidecar_path(&provider, 0);
        let actual_rows = get_nippy_row_count(&provider, 0);
        assert_eq!(get_sidecar_block_count(&sidecar_path), 10);

        // Simulate a scenario where sidecar has blocks pointing past actual rows.
        // Add 2 fake blocks that would exceed the row count.
        // Block 11: offset=actual_rows, count=5 (pointing past EOF)
        corrupt_sidecar_add_fake_blocks(&sidecar_path, 2, actual_rows);
        assert_eq!(get_sidecar_block_count(&sidecar_path), 12);

        // Reopen - healing validates all offsets against NippyJar.rows()
        drop(provider);
        let provider = setup_test_provider(&static_dir, 100);
        let _writer = provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();

        // After healing: sidecar clamped to header (10), rows unchanged
        assert_eq!(get_sidecar_block_count(&sidecar_path), 10, "Sidecar clamped to header");
        assert_eq!(get_header_block_count(&provider, 0), 10);
        assert_eq!(get_nippy_row_count(&provider, 0), actual_rows, "NippyJar rows unchanged");
    }

    #[test]
    fn test_prune_crash_sidecar_truncate_not_synced() {
        // PRUNE CP6: Sidecar truncated but not synced (power loss could resurrect old length).
        // SCENARIO: We pruned from 10 to 7 blocks. Header was updated to 7.
        // Sidecar was truncated to 7 but fsync didn't complete before power loss.
        // After restart, filesystem resurrects sidecar back to 10 (old length).
        //
        // State after crash: Header says 7, sidecar shows 10.
        // Expected: Healing clamps sidecar to header's 7 (header is authoritative).

        let (static_dir, _) = create_test_static_files_dir();
        let provider = setup_test_provider(&static_dir, 100);

        // Write 10 blocks with 5 changes each
        let sidecar_path = write_test_blocks(&provider, 10, 5);
        assert_eq!(get_sidecar_block_count(&sidecar_path), 10);
        assert_eq!(get_header_block_count(&provider, 0), 10);

        // Prune to 7 blocks (keep blocks 0-6)
        {
            let mut writer = provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();
            writer.prune_account_changesets(6).unwrap();
            writer.commit().unwrap();
        }

        // Verify prune worked
        drop(provider);
        let provider = setup_test_provider(&static_dir, 100);
        assert_eq!(get_header_block_count(&provider, 0), 7);
        assert_eq!(get_sidecar_block_count(&sidecar_path), 7);

        // Now write 3 more blocks (back to 10 total)
        {
            let mut writer = provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();
            for block_num in 7..10 {
                let changeset = generate_test_changeset(block_num, 5);
                writer.append_account_changeset(changeset, block_num).unwrap();
            }
            writer.commit().unwrap();
        }
        drop(provider);
        let provider = setup_test_provider(&static_dir, 100);
        assert_eq!(get_sidecar_block_count(&sidecar_path), 10);
        assert_eq!(get_header_block_count(&provider, 0), 10);

        // Prune back to 7 again
        {
            let mut writer = provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();
            writer.prune_account_changesets(6).unwrap();
            writer.commit().unwrap();
        }
        drop(provider);

        // SIMULATE POWER LOSS: Restore sidecar to 10 blocks (as if truncate wasn't synced)
        // Extend sidecar back to 10 blocks to simulate "resurrected" state
        corrupt_sidecar_add_fake_blocks(&sidecar_path, 3, 35);
        assert_eq!(get_sidecar_block_count(&sidecar_path), 10, "Simulated resurrected sidecar");

        // Reopen - healing should detect sidecar (10) > header (7) and clamp
        let provider = setup_test_provider(&static_dir, 100);
        let _writer = provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();

        // Header is authoritative - sidecar must be clamped to 7
        assert_eq!(get_sidecar_block_count(&sidecar_path), 7, "Sidecar clamped to header");
        assert_eq!(get_header_block_count(&provider, 0), 7, "Header unchanged");
    }

    #[test]
    fn test_prune_with_unflushed_current_offset() {
        // REGRESSION (Issue #7): truncate_changesets() didn't call flush_current_changeset_offset()
        // before reading the sidecar. This caused incorrect truncation when there was an unflushed
        // current_changeset_offset from recent appends.
        //
        // Test scenario: Prune immediately after appending without committing first.

        let (static_dir, _) = create_test_static_files_dir();
        let provider = setup_test_provider(&static_dir, 100);

        let sidecar_path = get_sidecar_path(&provider, 0);

        {
            let mut writer = provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();

            // Step 1: Write 5 blocks (0-4), commit
            for block_num in 0..5u64 {
                let changeset = generate_test_changeset(block_num, 3);
                writer.append_account_changeset(changeset, block_num).unwrap();
            }
            writer.commit().unwrap();

            // Step 2: Append 5 more blocks (5-9) WITHOUT committing
            for block_num in 5..10u64 {
                let changeset = generate_test_changeset(block_num, 3);
                writer.append_account_changeset(changeset, block_num).unwrap();
            }
            // Note: NOT committing here - block 9's offset is only in current_changeset_offset

            // Step 3: Prune to block 7 (should keep blocks 0-7, including uncommitted 5-7)
            // This tests that the unflushed current_changeset_offset (for block 9) is properly
            // flushed before reading the sidecar during truncation.
            writer.prune_account_changesets(7).unwrap();

            // Step 4: Commit
            writer.commit().unwrap();
        }

        // Step 5: Verify sidecar has 8 blocks, header has 8 blocks, rows are correct
        assert_eq!(get_sidecar_block_count(&sidecar_path), 8, "Sidecar should have 8 blocks (0-7)");
        assert_eq!(get_header_block_count(&provider, 0), 8, "Header should have 8 blocks");
        assert_eq!(
            get_nippy_row_count(&provider, 0),
            24,
            "Should have 8 blocks * 3 changes = 24 rows"
        );

        // Verify stability after reopen
        drop(provider);
        let provider = setup_test_provider(&static_dir, 100);
        let _writer = provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();

        assert_eq!(get_sidecar_block_count(&sidecar_path), 8, "Sidecar stable after reopen");
        assert_eq!(get_header_block_count(&provider, 0), 8, "Header stable after reopen");
        assert_eq!(get_nippy_row_count(&provider, 0), 24, "Rows stable after reopen");
    }

    #[test]
    fn test_prune_clean_no_crash() {
        // BASELINE: Normal prune with no crash.

        let (static_dir, _) = create_test_static_files_dir();
        let provider = setup_test_provider(&static_dir, 100);

        // Write 10 blocks
        write_test_blocks(&provider, 10, 5);

        // Prune to keep blocks 0-6 (7 blocks total, removing blocks 7-9)
        // prune_account_changesets takes last_block to keep, not count to remove
        let sidecar_path = get_sidecar_path(&provider, 0);
        {
            let mut writer = provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();
            writer.prune_account_changesets(6).unwrap(); // Keep blocks 0-6
            writer.commit().unwrap();
        }

        // Reopen provider to see committed changes
        drop(provider);
        let provider = setup_test_provider(&static_dir, 100);

        // Verify all in sync at 7 blocks (0-6)
        assert_eq!(get_sidecar_block_count(&sidecar_path), 7);
        assert_eq!(get_header_block_count(&provider, 0), 7);

        // Rows should also be reduced (7 blocks * 5 changes = 35 rows)
        assert_eq!(get_nippy_row_count(&provider, 0), 35);
    }

    // ==================== VALIDATION EDGE CASES ====================

    #[test]
    fn test_empty_segment_fresh_start() {
        // SCENARIO: Brand new segment with no data.

        let (static_dir, _) = create_test_static_files_dir();
        let provider = setup_test_provider(&static_dir, 100);

        // Just open a writer without writing anything
        {
            let _writer = provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();
        }

        // Sidecar might not exist or be empty
        let sidecar_path = get_sidecar_path(&provider, 0);
        let block_count = get_sidecar_block_count(&sidecar_path);
        assert_eq!(block_count, 0, "Fresh segment should have 0 blocks");
    }

    #[test]
    fn test_all_empty_blocks_preserved_on_reopen() {
        // REGRESSION: When all blocks have empty changesets (0 rows in NippyJar),
        // healing incorrectly pruned all blocks because the validation was skipped
        // when actual_nippy_rows == 0. But (offset=0, num_changes=0) is valid when rows=0.

        let (static_dir, _) = create_test_static_files_dir();
        let provider = setup_test_provider(&static_dir, 100);

        // Write 5 blocks with 0 changes each => 0 total rows in NippyJar
        let sidecar_path = write_test_blocks(&provider, 5, 0);
        assert_eq!(get_sidecar_block_count(&sidecar_path), 5);
        assert_eq!(get_header_block_count(&provider, 0), 5);
        assert_eq!(get_nippy_row_count(&provider, 0), 0);

        // Reopen - healing must preserve all 5 blocks
        drop(provider);
        let provider = setup_test_provider(&static_dir, 100);
        let _writer = provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();

        // Must still have 5 blocks after healing
        assert_eq!(
            get_sidecar_block_count(&sidecar_path),
            5,
            "Healing should not prune empty blocks"
        );
        assert_eq!(get_header_block_count(&provider, 0), 5);
        assert_eq!(get_nippy_row_count(&provider, 0), 0);
    }

    #[test]
    fn test_empty_blocks_zero_changes() {
        // SCENARIO: Some blocks have 0 changes (empty changesets).

        let (static_dir, _) = create_test_static_files_dir();
        let provider = setup_test_provider(&static_dir, 100);

        {
            let mut writer = provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();

            // Block 0: 3 changes
            writer.append_account_changeset(generate_test_changeset(0, 3), 0).unwrap();

            // Block 1: 0 changes (empty)
            writer.append_account_changeset(vec![], 1).unwrap();

            // Block 2: 2 changes
            writer.append_account_changeset(generate_test_changeset(2, 2), 2).unwrap();

            // Block 3: 0 changes (empty)
            writer.append_account_changeset(vec![], 3).unwrap();

            // Block 4: 5 changes
            writer.append_account_changeset(generate_test_changeset(4, 5), 4).unwrap();

            writer.commit().unwrap();
        }

        let sidecar_path = get_sidecar_path(&provider, 0);

        // Verify 5 blocks recorded
        assert_eq!(get_sidecar_block_count(&sidecar_path), 5);
        assert_eq!(get_header_block_count(&provider, 0), 5);

        // Verify offsets are correct
        let mut reader = ChangesetOffsetReader::new(&sidecar_path, 5).unwrap();

        let o0 = reader.get(0).unwrap().unwrap();
        assert_eq!(o0.offset(), 0);
        assert_eq!(o0.num_changes(), 3);

        let o1 = reader.get(1).unwrap().unwrap();
        assert_eq!(o1.offset(), 3);
        assert_eq!(o1.num_changes(), 0); // Empty block

        let o2 = reader.get(2).unwrap().unwrap();
        assert_eq!(o2.offset(), 3); // Same offset as block 1 (0 changes didn't advance)
        assert_eq!(o2.num_changes(), 2);

        let o3 = reader.get(3).unwrap().unwrap();
        assert_eq!(o3.offset(), 5);
        assert_eq!(o3.num_changes(), 0); // Empty block

        let o4 = reader.get(4).unwrap().unwrap();
        assert_eq!(o4.offset(), 5);
        assert_eq!(o4.num_changes(), 5);

        // Total rows: 3 + 0 + 2 + 0 + 5 = 10
        assert_eq!(get_nippy_row_count(&provider, 0), 10);

        // Reopen and verify healing doesn't break empty blocks
        drop(provider);
        let provider = setup_test_provider(&static_dir, 100);
        let _writer = provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();
        assert_eq!(get_sidecar_block_count(&sidecar_path), 5);
    }

    #[test]
    fn test_healing_never_enlarges_header() {
        // INVARIANT: Header is the commit marker. Healing should NEVER enlarge it.
        // Even if sidecar has more valid blocks, we trust header.

        let (static_dir, _) = create_test_static_files_dir();
        let provider = setup_test_provider(&static_dir, 100);

        // Write 5 blocks
        let sidecar_path = write_test_blocks(&provider, 5, 3);

        // Add more blocks to sidecar that would be "valid" (point within existing rows)
        // These simulate uncommitted blocks from a crashed append
        corrupt_sidecar_add_fake_blocks(&sidecar_path, 3, 0);
        assert_eq!(get_sidecar_block_count(&sidecar_path), 8);

        // Reopen - healing should clamp to header's 5, not enlarge to 8
        drop(provider);
        let provider = setup_test_provider(&static_dir, 100);
        let _writer = provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();

        assert_eq!(get_sidecar_block_count(&sidecar_path), 5, "Must clamp to header");
        assert_eq!(get_header_block_count(&provider, 0), 5, "Header unchanged");
    }

    #[test]
    fn test_multiple_reopen_cycles_stable() {
        // STABILITY: Opening and closing multiple times shouldn't change anything.

        let (static_dir, _) = create_test_static_files_dir();

        // Initial write
        {
            let provider = setup_test_provider(&static_dir, 100);
            write_test_blocks(&provider, 10, 5);
        }

        // Reopen 5 times
        for i in 0..5 {
            let provider = setup_test_provider(&static_dir, 100);
            let sidecar_path = get_sidecar_path(&provider, 0);

            let _writer = provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();

            assert_eq!(get_sidecar_block_count(&sidecar_path), 10, "Cycle {}: block count", i);
            assert_eq!(get_header_block_count(&provider, 0), 10, "Cycle {}: header", i);
            assert_eq!(get_nippy_row_count(&provider, 0), 50, "Cycle {}: rows", i);
        }
    }

    #[test]
    fn test_combined_partial_and_extra_blocks() {
        // COMBINED: Partial record AND extra complete blocks.
        // Healing should handle both: truncate partial, then validate remaining.

        let (static_dir, _) = create_test_static_files_dir();
        let provider = setup_test_provider(&static_dir, 100);

        // Write 5 blocks
        let sidecar_path = write_test_blocks(&provider, 5, 3);

        // Corrupt: add 2 fake blocks pointing past EOF, then partial record
        corrupt_sidecar_add_fake_blocks(&sidecar_path, 2, 100); // Invalid offsets
        corrupt_sidecar_partial_write(&sidecar_path, 10); // Partial record

        let file_size = std::fs::metadata(&sidecar_path).unwrap().len();
        assert_eq!(file_size, 5 * 16 + 2 * 16 + 10, "5 valid + 2 fake + 10 partial");

        // Reopen - healing should:
        // 1. Truncate partial (10 bytes)
        // 2. Clamp to header's 5 blocks (remove 2 fake blocks)
        drop(provider);
        let provider = setup_test_provider(&static_dir, 100);
        let _writer = provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();

        assert_eq!(get_sidecar_block_count(&sidecar_path), 5);
        assert_eq!(get_header_block_count(&provider, 0), 5);
    }

    // ==================== REGRESSION TESTS ====================

    #[test]
    fn test_prune_double_decrement_regression() {
        // REGRESSION: Previously, healing called set_changeset_offsets_len() then prune(),
        // causing double decrement. Now we only call prune() which handles both.

        let (static_dir, _) = create_test_static_files_dir();
        let provider = setup_test_provider(&static_dir, 100);

        // Write 10 blocks
        let sidecar_path = write_test_blocks(&provider, 10, 5);

        // Simulate prune crash: sidecar at 7, header should be updated from 10 to 7
        truncate_sidecar(&sidecar_path, 7);

        // Reopen - healing fixes header
        drop(provider);
        let provider = setup_test_provider(&static_dir, 100);
        {
            let _writer = provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();
            // Writer commits healed header on open
        }
        // Reopen provider to see committed changes
        drop(provider);
        let provider = setup_test_provider(&static_dir, 100);

        // Should be exactly 7, not 7 - (10-7) = 4 (double decrement bug)
        assert_eq!(get_header_block_count(&provider, 0), 7);
        assert_eq!(get_sidecar_block_count(&sidecar_path), 7);
    }

    #[test]
    fn test_prune_with_uncommitted_sidecar_records() {
        // REGRESSION: truncate_changesets() previously read file size from disk instead of
        // using the committed length from header. After a crash, sidecar may have uncommitted
        // records. The fix uses ChangesetOffsetReader::new() with explicit length.
        //
        // SCENARIO: Simulate crash where sidecar has more records than header claims,
        // then prune. The prune should use header's block count, not sidecar's.

        let (static_dir, _) = create_test_static_files_dir();
        let provider = setup_test_provider(&static_dir, 100);

        // Step 1: Write 10 blocks with 5 changes each, commit
        let sidecar_path = write_test_blocks(&provider, 10, 5);
        assert_eq!(get_sidecar_block_count(&sidecar_path), 10);
        assert_eq!(get_header_block_count(&provider, 0), 10);
        assert_eq!(get_nippy_row_count(&provider, 0), 50);

        // Step 2: Corrupt sidecar by adding 3 fake blocks that point to valid but
        // uncommitted offsets. This simulates a crash where sidecar was synced but
        // header wasn't committed.
        corrupt_sidecar_add_fake_blocks(&sidecar_path, 3, 50);
        assert_eq!(get_sidecar_block_count(&sidecar_path), 13, "Sidecar has 3 uncommitted blocks");

        // Step 3: Reopen provider - healing runs and clamps sidecar to header's 10
        drop(provider);
        let provider = setup_test_provider(&static_dir, 100);
        {
            let _writer = provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();
        }
        assert_eq!(get_sidecar_block_count(&sidecar_path), 10, "Healing clamped sidecar to 10");
        assert_eq!(get_header_block_count(&provider, 0), 10);

        // Step 4: Prune to block 6 (keep blocks 0-6, remove 7-9)
        {
            let mut writer = provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();
            writer.prune_account_changesets(6).unwrap();
            writer.commit().unwrap();
        }

        // Step 5: Verify sidecar has 7 blocks, header has 7 blocks, rows are correct
        drop(provider);
        let provider = setup_test_provider(&static_dir, 100);

        assert_eq!(get_sidecar_block_count(&sidecar_path), 7, "Sidecar should have 7 blocks");
        assert_eq!(get_header_block_count(&provider, 0), 7, "Header should have 7 blocks");
        assert_eq!(
            get_nippy_row_count(&provider, 0),
            35,
            "Should have 7 blocks * 5 changes = 35 rows"
        );
    }
}

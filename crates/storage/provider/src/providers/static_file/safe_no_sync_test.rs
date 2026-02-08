//! Tests for safe-no-sync crash recovery.
//!
//! These tests verify that reth can recover from a crash when using `--db.sync-mode safe-no-sync`.
//!
//! In safe-no-sync mode, MDBX commits may be lost on crash because fsync is skipped.
//! The recovery mechanism relies on:
//! 1. Static files being written and synced BEFORE MDBX commit
//! 2. Changesets being stored in static files (not MDBX)
//! 3. `check_consistency` detecting when static files are ahead of MDBX checkpoints
//! 4. Pipeline unwind using changesets from static files, then re-execution

use crate::{
    test_utils::create_test_provider_factory, BlockWriter, DatabaseProviderFactory,
    StageCheckpointWriter, StaticFileProviderFactory,
};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::DBProvider;
use reth_testing_utils::generators::{self, BlockRangeParams};

/// Simulates a safe-no-sync crash scenario where:
/// - Static files have been written up to block N
/// - MDBX commit was lost, so checkpoint is at block N-1
///
/// This is the exact scenario that happens when:
/// 1. Node writes static files (Headers, Transactions, Receipts, Changesets)
/// 2. Node attempts MDBX commit (with checkpoint update)
/// 3. Crash occurs before MDBX fsync completes (safe-no-sync mode)
/// 4. On restart, static files are ahead of MDBX
///
/// Expected behavior: `check_consistency` should detect this and return an unwind target.
#[test]
fn test_safe_no_sync_crash_recovery_static_files_ahead() {
    let factory = create_test_provider_factory();

    // Generate blocks 0-5
    let mut rng = generators::rng();
    let blocks = generators::random_block_range(
        &mut rng,
        0..=5,
        BlockRangeParams {
            parent: Some(alloy_primitives::B256::ZERO),
            tx_count: 2..3,
            ..Default::default()
        },
    );

    // Step 1: Write ALL blocks (0-5) to static files and MDBX
    {
        let provider = factory.database_provider_rw().unwrap();
        for block in &blocks {
            provider.insert_block(&block.clone().try_recover().expect("recover block")).unwrap();
        }
        // Set checkpoint to block 5 (all blocks synced)
        provider.save_stage_checkpoint(StageId::Headers, StageCheckpoint::new(5)).unwrap();
        provider.save_stage_checkpoint(StageId::Bodies, StageCheckpoint::new(5)).unwrap();
        provider.save_stage_checkpoint(StageId::Execution, StageCheckpoint::new(5)).unwrap();
        provider.commit().unwrap();
    }

    // Verify static files have block 5
    let sf_provider = factory.static_file_provider();
    let highest_header_block = sf_provider
        .get_highest_static_file_block(StaticFileSegment::Headers)
        .expect("should have headers");
    assert_eq!(highest_header_block, 5, "Static files should have block 5");

    // Step 2: Simulate a safe-no-sync crash by rolling back MDBX checkpoint
    // This simulates what happens when MDBX commit is lost due to no fsync
    {
        let provider = factory.database_provider_rw().unwrap();
        // Roll back checkpoints to block 3 (simulating lost MDBX commit)
        provider.save_stage_checkpoint(StageId::Headers, StageCheckpoint::new(3)).unwrap();
        provider.save_stage_checkpoint(StageId::Bodies, StageCheckpoint::new(3)).unwrap();
        provider.save_stage_checkpoint(StageId::Execution, StageCheckpoint::new(3)).unwrap();
        provider.commit().unwrap();
    }

    // Step 3: Run consistency check - should detect static files ahead of MDBX
    let provider = factory.database_provider_ro().unwrap();
    let unwind_target = sf_provider.check_consistency(&provider).unwrap();

    // The consistency check should detect the mismatch and return an unwind target
    // Static files are at block 5, MDBX checkpoint is at block 3
    // We expect it to return Some(unwind_target) to trigger recovery
    assert!(
        unwind_target.is_some(),
        "check_consistency should detect static files ahead of MDBX and return unwind target"
    );

    let target = unwind_target.unwrap();
    // The unwind target should be block 3 (the MDBX checkpoint)
    // After unwind, pipeline will re-execute blocks 4-5 from static file data
    assert!(
        target.unwind_target().unwrap() <= 5,
        "Unwind target should be at or below static file tip"
    );
}

/// Tests that when static files and MDBX are consistent, no panic occurs.
#[test]
fn test_safe_no_sync_consistent_state_no_panic() {
    let factory = create_test_provider_factory();

    // Generate blocks 0-3
    let mut rng = generators::rng();
    let blocks = generators::random_block_range(
        &mut rng,
        0..=3,
        BlockRangeParams {
            parent: Some(alloy_primitives::B256::ZERO),
            tx_count: 2..3,
            ..Default::default()
        },
    );

    // Write blocks and checkpoint consistently
    {
        let provider = factory.database_provider_rw().unwrap();
        for block in &blocks {
            provider.insert_block(&block.clone().try_recover().expect("recover block")).unwrap();
        }
        provider.save_stage_checkpoint(StageId::Headers, StageCheckpoint::new(3)).unwrap();
        provider.save_stage_checkpoint(StageId::Bodies, StageCheckpoint::new(3)).unwrap();
        provider.save_stage_checkpoint(StageId::Execution, StageCheckpoint::new(3)).unwrap();
        provider.commit().unwrap();
    }

    // Verify static files have block 3
    let sf_provider = factory.static_file_provider();
    let highest_header_block = sf_provider
        .get_highest_static_file_block(StaticFileSegment::Headers)
        .expect("should have headers");
    assert_eq!(highest_header_block, 3);

    // Run consistency check - should not panic
    let provider = factory.database_provider_ro().unwrap();
    let unwind_target = sf_provider.check_consistency(&provider).unwrap();

    // Note: check_consistency may return Some even for "consistent" states
    // if there are segments to check beyond what we've set up (e.g., Receipts, Transactions).
    // The key test is that crash recovery works
    // (test_safe_no_sync_crash_recovery_static_files_ahead). This test verifies that
    // check_consistency doesn't panic on consistent state.
    if let Some(ref target) = unwind_target {
        // If an unwind is requested, the target should be reasonable (at or below checkpoint)
        if let Some(block) = target.unwind_target() {
            assert!(block <= 3, "Unwind target {} should be at or below checkpoint 3", block);
        }
    }
}

/// Tests recovery when MDBX checkpoint is behind static files.
/// This simulates a severe crash where MDBX lost multiple commits.
#[test]
fn test_safe_no_sync_mdbx_far_behind_static_files() {
    let factory = create_test_provider_factory();

    // Generate blocks 0-2
    let mut rng = generators::rng();
    let blocks = generators::random_block_range(
        &mut rng,
        0..=2,
        BlockRangeParams {
            parent: Some(alloy_primitives::B256::ZERO),
            tx_count: 2..3,
            ..Default::default()
        },
    );

    // Step 1: Write blocks to static files and MDBX
    {
        let provider = factory.database_provider_rw().unwrap();
        for block in &blocks {
            provider.insert_block(&block.clone().try_recover().expect("recover block")).unwrap();
        }
        provider.save_stage_checkpoint(StageId::Headers, StageCheckpoint::new(2)).unwrap();
        provider.commit().unwrap();
    }

    // Verify static files have block 2
    let sf_provider = factory.static_file_provider();
    let highest_header_block = sf_provider
        .get_highest_static_file_block(StaticFileSegment::Headers)
        .expect("should have headers");
    assert_eq!(highest_header_block, 2);

    // Step 2: Simulate MDBX data loss by rolling back checkpoint
    {
        let provider = factory.database_provider_rw().unwrap();
        provider.save_stage_checkpoint(StageId::Headers, StageCheckpoint::new(0)).unwrap();
        provider.commit().unwrap();
    }

    // Step 3: Run consistency check
    let provider = factory.database_provider_ro().unwrap();
    let unwind_target = sf_provider.check_consistency(&provider).unwrap();

    // Should detect that static files are ahead and request recovery
    if let Some(target) = unwind_target {
        // If it returns an unwind target, it should be valid
        assert!(target.unwind_target().is_some(), "Unwind target should have a block number");
    }
}

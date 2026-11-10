//! Durable PBT checkpoint and branch recovery tests.

#![cfg(feature = "persistence")]

use alloy_primitives::{Address, B256, U256};
use reth_trie_pbt::{
    migration::{mpt_root, ArtifactError, Artifacts, DecodeLimits},
    persistence::{PruneOutcome, SnapshotStore, StoreError},
    AccountData, Blake3Hasher, StateTree,
};

#[test]
fn checkpoints_survive_reopening_and_keep_competing_branches() {
    let directory = tempfile::tempdir().unwrap();
    let store =
        SnapshotStore::<Blake3Hasher>::open(directory.path(), DecodeLimits::default()).unwrap();
    let a = B256::repeat_byte(1);
    let b = B256::repeat_byte(2);
    assert!(!store.contains_block(a).unwrap());
    let mut state = StateTree::<Blake3Hasher>::new();
    state
        .set_account(
            Address::ZERO,
            AccountData { nonce: 1, balance: U256::from(2), code: Default::default() },
        )
        .unwrap();
    let a_root = store.save_block(a, &state).unwrap();
    assert!(store.contains_block(a).unwrap());
    state.set_storage(Address::ZERO, U256::from(64), B256::repeat_byte(3)).unwrap();
    let b_root = store.save_block(b, &state).unwrap();
    drop(store);
    let reopened =
        SnapshotStore::<Blake3Hasher>::open(directory.path(), DecodeLimits::default()).unwrap();
    assert_eq!(reopened.load_block(a).unwrap().root(), a_root);
    assert_eq!(reopened.load_block(b).unwrap().root(), b_root);
    assert!(reopened.load(B256::repeat_byte(9)).is_err());
    reth_fs_util::write(directory.path().join("states").join(b_root.to_string()), [0; 7]).unwrap();
    drop(reopened);
    let reopened =
        SnapshotStore::<Blake3Hasher>::open(directory.path(), DecodeLimits::default()).unwrap();
    assert!(reopened.load_block(b).is_err());
    assert_eq!(reopened.load_block(a).unwrap().root(), a_root);
}

#[test]
fn store_rejects_state_it_could_not_restore_under_its_limits() {
    let directory = tempfile::tempdir().unwrap();
    let store = SnapshotStore::<Blake3Hasher>::open(
        directory.path(),
        DecodeLimits { max_leaves: 1, ..Default::default() },
    )
    .unwrap();
    let mut state = StateTree::<Blake3Hasher>::new();
    state.set_account(Address::ZERO, AccountData { nonce: 1, ..Default::default() }).unwrap();
    assert!(store.save_block(B256::ZERO, &state).is_err());
    assert!(store.load_block(B256::ZERO).is_err());
}

#[test]
fn oversized_checkpoints_and_references_are_rejected_before_reading() {
    let directory = tempfile::tempdir().unwrap();
    let store = SnapshotStore::<Blake3Hasher>::open(
        directory.path(),
        DecodeLimits { max_accounts: 0, max_leaves: 0, ..Default::default() },
    )
    .unwrap();
    let path = directory.path().join("states").join(B256::ZERO.to_string());
    // A sparse corrupt checkpoint must fail its metadata bound without allocating its length.
    let file = reth_fs_util::create_file(&path).unwrap();
    file.set_len(1 << 30).unwrap();
    assert!(matches!(
        store.load(B256::ZERO),
        Err(StoreError::Artifact(ArtifactError::ResourceLimit))
    ));
    reth_fs_util::write(&path, u64::MAX.to_be_bytes()).unwrap();
    assert!(matches!(
        store.load(B256::ZERO),
        Err(StoreError::Artifact(ArtifactError::ResourceLimit))
    ));
    reth_fs_util::write(directory.path().join("blocks").join(B256::ZERO.to_string()), [0; 33])
        .unwrap();
    assert!(matches!(
        store.load_block(B256::ZERO),
        Err(StoreError::Artifact(ArtifactError::ResourceLimit))
    ));
}

#[test]
fn checkpoints_restore_under_tight_leaf_and_account_bounds() {
    let directory = tempfile::tempdir().unwrap();
    let mut state = account_state(1);
    state.set_storage(Address::ZERO, U256::from(63), B256::repeat_byte(1)).unwrap();
    state.set_storage(Address::ZERO, U256::from(64), B256::repeat_byte(2)).unwrap();
    let limits = DecodeLimits {
        max_accounts: 1,
        max_leaves: state.tree().len(),
        max_code_size: 0,
        max_total_code_size: 0,
    };
    let store = SnapshotStore::<Blake3Hasher>::open(directory.path(), limits).unwrap();
    let root = store.save_block(B256::ZERO, &state).unwrap();
    drop(store);
    let store = SnapshotStore::<Blake3Hasher>::open(directory.path(), limits).unwrap();
    assert_eq!(store.load_block(B256::ZERO).unwrap().root(), root);
}

#[test]
fn imported_anchor_is_dual_verified_and_survives_reopening() {
    let directory = tempfile::tempdir().unwrap();
    let state = account_state(2);
    let artifacts = Artifacts::from_state(&state).unwrap();
    let snapshot = directory.path().join("anchor.snapshot");
    let preimages = directory.path().join("anchor.preimages");
    reth_fs_util::write(&snapshot, &artifacts.snapshot).unwrap();
    reth_fs_util::write(&preimages, &artifacts.preimages).unwrap();
    let store =
        SnapshotStore::<Blake3Hasher>::open(directory.path(), DecodeLimits::default()).unwrap();
    let block = B256::repeat_byte(1);
    assert_eq!(
        store.import_anchor(block, mpt_root(&state), &snapshot, &preimages).unwrap(),
        state.root()
    );
    assert!(matches!(
        store.import_anchor(B256::repeat_byte(2), B256::ZERO, &snapshot, &preimages),
        Err(StoreError::Artifact(ArtifactError::AnchorRootMismatch))
    ));
    assert!(!directory.path().join("blocks").join(B256::repeat_byte(2).to_string()).exists());
    drop(store);
    let store =
        SnapshotStore::<Blake3Hasher>::open(directory.path(), DecodeLimits::default()).unwrap();
    assert_eq!(store.load_block(block).unwrap().root(), state.root());
}

#[test]
fn imported_artifact_files_are_individually_bounded() {
    let directory = tempfile::tempdir().unwrap();
    let state = StateTree::<Blake3Hasher>::new();
    let artifacts = Artifacts::from_state(&state).unwrap();
    let snapshot = directory.path().join("anchor.snapshot");
    let preimages = directory.path().join("anchor.preimages");
    let store = SnapshotStore::<Blake3Hasher>::open(
        directory.path(),
        DecodeLimits { max_accounts: 0, max_leaves: 0, ..Default::default() },
    )
    .unwrap();
    for oversized_snapshot in [true, false] {
        reth_fs_util::write(&snapshot, &artifacts.snapshot).unwrap();
        reth_fs_util::write(&preimages, &artifacts.preimages).unwrap();
        let path = if oversized_snapshot { &snapshot } else { &preimages };
        reth_fs_util::create_file(path).unwrap().set_len(1 << 30).unwrap();
        assert!(matches!(
            store.import_anchor(B256::ZERO, mpt_root(&state), &snapshot, &preimages),
            Err(StoreError::Artifact(ArtifactError::ResourceLimit))
        ));
        assert!(!directory.path().join("blocks").join(B256::ZERO.to_string()).exists());
    }
}

#[test]
fn pruning_preserves_finalized_state_branches_shared_roots_and_unreferenced_states() {
    let directory = tempfile::tempdir().unwrap();
    let store =
        SnapshotStore::<Blake3Hasher>::open(directory.path(), DecodeLimits::default()).unwrap();
    let obsolete = B256::repeat_byte(1);
    let obsolete_shared = B256::repeat_byte(2);
    let finalized = B256::repeat_byte(3);
    let branch = B256::repeat_byte(4);
    let child = B256::repeat_byte(5);
    let obsolete_root = store.save_block(obsolete, &account_state(1)).unwrap();
    let shared_root = store.save_block(obsolete_shared, &account_state(2)).unwrap();
    let finalized_root = store.save_block(finalized, &account_state(3)).unwrap();
    store.save_block(branch, &account_state(2)).unwrap();
    let child_root = store.save_block(child, &account_state(4)).unwrap();
    let unreferenced_root = store.save(&account_state(5)).unwrap();
    assert_eq!(
        store.prune_finalized(finalized, &[obsolete, obsolete, obsolete_shared]).unwrap(),
        PruneOutcome { block_references: 2, state_files: 1 }
    );
    assert!(matches!(
        store.load(obsolete_root),
        Err(StoreError::Io(reth_fs_util::FsPathError::Open { source, .. }))
            if source.kind() == std::io::ErrorKind::NotFound
    ));
    assert!(!directory.path().join("blocks").join(obsolete.to_string()).exists());
    assert!(!directory.path().join("blocks").join(obsolete_shared.to_string()).exists());
    drop(store);
    let store =
        SnapshotStore::<Blake3Hasher>::open(directory.path(), DecodeLimits::default()).unwrap();
    assert_eq!(store.load_block(finalized).unwrap().root(), finalized_root);
    assert_eq!(store.load_block(branch).unwrap().root(), shared_root);
    assert_eq!(store.load_block(child).unwrap().root(), child_root);
    assert_eq!(store.load(unreferenced_root).unwrap().root(), unreferenced_root);
}

#[test]
fn pruning_cannot_remove_the_finalized_reference_or_its_shared_state() {
    let directory = tempfile::tempdir().unwrap();
    let store =
        SnapshotStore::<Blake3Hasher>::open(directory.path(), DecodeLimits::default()).unwrap();
    let obsolete = B256::repeat_byte(1);
    let finalized = B256::repeat_byte(2);
    let state = account_state(1);
    store.save_block(obsolete, &state).unwrap();
    store.save_block(finalized, &state).unwrap();
    assert!(matches!(
        store.prune_finalized(finalized, &[finalized, obsolete]),
        Err(StoreError::PruneFinalized)
    ));
    assert_eq!(store.load_block(obsolete).unwrap().root(), state.root());
    assert_eq!(
        store.prune_finalized(finalized, &[obsolete]).unwrap(),
        PruneOutcome { block_references: 1, state_files: 0 }
    );
    assert_eq!(store.load_block(finalized).unwrap().root(), state.root());
}

#[test]
fn pruning_requires_every_requested_reference_before_deleting_anything() {
    let directory = tempfile::tempdir().unwrap();
    let store =
        SnapshotStore::<Blake3Hasher>::open(directory.path(), DecodeLimits::default()).unwrap();
    let obsolete = B256::repeat_byte(1);
    let finalized = B256::repeat_byte(2);
    let missing = B256::repeat_byte(3);
    let obsolete_root = store.save_block(obsolete, &account_state(1)).unwrap();
    store.save_block(finalized, &account_state(2)).unwrap();
    for (finalized, candidates) in [(missing, vec![obsolete]), (finalized, vec![obsolete, missing])]
    {
        assert!(matches!(
            store.prune_finalized(finalized, &candidates),
            Err(StoreError::MissingBlockReference(hash)) if hash == missing
        ));
        assert_eq!(store.load_block(obsolete).unwrap().root(), obsolete_root);
    }
}

#[test]
fn corrupt_retained_reference_stops_pruning_before_any_deletion() {
    let directory = tempfile::tempdir().unwrap();
    let store =
        SnapshotStore::<Blake3Hasher>::open(directory.path(), DecodeLimits::default()).unwrap();
    let obsolete = B256::repeat_byte(1);
    let finalized = B256::repeat_byte(2);
    let branch = B256::repeat_byte(3);
    let obsolete_root = store.save_block(obsolete, &account_state(1)).unwrap();
    store.save_block(finalized, &account_state(2)).unwrap();
    reth_fs_util::write(directory.path().join("blocks").join(branch.to_string()), [0; 31]).unwrap();
    assert!(matches!(
        store.prune_finalized(finalized, &[obsolete]),
        Err(StoreError::Artifact(ArtifactError::Invalid("invalid block checkpoint reference")))
    ));
    assert_eq!(store.load_block(obsolete).unwrap().root(), obsolete_root);
}

#[test]
fn pruning_reverifies_durable_finalized_and_candidate_states_even_when_cached() {
    for corrupt_finalized in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let store =
            SnapshotStore::<Blake3Hasher>::open(directory.path(), DecodeLimits::default()).unwrap();
        let obsolete = B256::repeat_byte(1);
        let finalized = B256::repeat_byte(2);
        let obsolete_root = store.save_block(obsolete, &account_state(1)).unwrap();
        let finalized_root = store.save_block(finalized, &account_state(2)).unwrap();
        let corrupted = if corrupt_finalized { finalized_root } else { obsolete_root };
        reth_fs_util::write(directory.path().join("states").join(corrupted.to_string()), [0; 7])
            .unwrap();
        assert!(matches!(
            store.prune_finalized(finalized, &[obsolete]),
            Err(StoreError::Artifact(ArtifactError::Truncated))
        ));
        assert!(directory.path().join("blocks").join(obsolete.to_string()).exists());
        assert!(directory.path().join("states").join(obsolete_root.to_string()).exists());
        assert!(directory.path().join("blocks").join(finalized.to_string()).exists());
    }
}

fn account_state(balance: u64) -> StateTree {
    let mut state = StateTree::new();
    state
        .set_account(
            Address::ZERO,
            AccountData { nonce: 1, balance: U256::from(balance), code: Default::default() },
        )
        .unwrap();
    state
}

#[test]
fn legacy_checkpoints_upgrade_and_interrupted_markers_recover() {
    let directory = tempfile::tempdir().unwrap();
    let state = account_state(1);
    let root = state.root();
    let artifacts = Artifacts::from_state(&state).unwrap();
    let store =
        SnapshotStore::<Blake3Hasher>::open(directory.path(), DecodeLimits::default()).unwrap();
    assert!(store.is_empty().unwrap());
    let path = directory.path().join("states").join(root.to_string());
    let legacy = [
        u64::try_from(artifacts.preimages.len()).unwrap().to_be_bytes().as_slice(),
        &artifacts.preimages,
        &artifacts.snapshot,
    ]
    .concat();
    reth_fs_util::write(&path, &legacy).unwrap();
    assert!(!store.is_empty().unwrap());
    assert_eq!(store.load(root).unwrap().root(), root);
    assert_eq!(reth_fs_util::metadata(&path).unwrap().len(), 12);
    drop(store);
    // The native root has reached the WAL, but the legacy file replacement was interrupted.
    reth_fs_util::write(&path, &legacy).unwrap();
    let store =
        SnapshotStore::<Blake3Hasher>::open(directory.path(), DecodeLimits::default()).unwrap();
    assert_eq!(store.load(root).unwrap().root(), root);
    assert_eq!(reth_fs_util::metadata(&path).unwrap().len(), 12);
    drop(store);
    reth_fs_util::remove_file(&path).unwrap();
    let store =
        SnapshotStore::<Blake3Hasher>::open(directory.path(), DecodeLimits::default()).unwrap();
    assert!(!store.is_empty().unwrap());
    assert_eq!(store.load(root).unwrap().root(), root);
    assert_eq!(reth_fs_util::metadata(&path).unwrap().len(), 12);
}

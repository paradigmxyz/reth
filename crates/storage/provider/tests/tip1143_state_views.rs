//! T-018/T-019: code identity across shared owners, snapshots, indexed history,
//! production unwind, appended execution overlays, and retained caches.

use alloy_consensus::Header;
use alloy_primitives::{keccak256, Address, Bytes, U256};
use reth_chain_state::ExecutedBlock;
use reth_db::test_utils::DatabaseTestHooks;
use reth_db_api::{
    models::{AccountBeforeTx, ShardedKey},
    tables,
    transaction::{DbTx, DbTxMut},
    BlockNumberList,
};
use reth_ethereum_primitives::{Block, EthPrimitives};
use reth_execution_cache::{CachedStateProvider, ExecutionCache};
use reth_execution_types::BlockExecutionOutput;
use reth_primitives_traits::{Account, RecoveredBlock};
use reth_provider::{
    test_utils::{create_test_provider_factory, create_test_provider_factory_with_db_hooks},
    ProviderError, StateProviderFactory,
};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_storage_api::{
    AccountReader, BlockWriter, BytecodeReader, CodeChunkReader, CodeValidationError, DBProvider,
    DatabaseProviderROFactory, StageCheckpointWriter, StateWriter, ValidatedCode,
};
use reth_storage_overlay::{OverlayManager, OverlayStateProviderFactory};
use revm::{bytecode::Bytecode as RevmBytecode, database::BundleState, state::AccountInfo};
use std::sync::Arc;

#[test]
fn t019_shared_code_survives_owner_replacement_and_clear() {
    let factory = create_test_provider_factory();
    let owners = [Address::repeat_byte(1), Address::repeat_byte(2)];
    // Two identical full chunks exercise repeated commitments within one descriptor.
    let mut original = vec![0; 49083];
    original[0] = 0x60;
    original[1] = 0x42;
    original[24541] = 0x60;
    original[24542] = 0x42;
    original[49082] = 0x01;
    let original = Bytes::from(original);
    let hash = keccak256(&original);
    let code = ValidatedCode::new(original.clone()).unwrap();
    let account = Account {
        nonce: 17,
        balance: U256::from(29),
        bytecode_hash: Some(hash),
        extension: Bytes::from_static(&[0xfe, 0x11, 0x43]).into(),
    };
    for _ in 0..3 {
        let writer = factory.provider_rw().unwrap();
        for owner in owners {
            writer.write_chunked_code(owner, account.clone(), &code).unwrap();
        }
        writer.commit().unwrap();
    }
    let snapshot = factory.latest().unwrap();
    // A different full-code identity shares the first payload, but has a distinct tail.
    let mut sibling = original[..24541].to_vec();
    sibling.push(0x5b);
    let sibling = Bytes::from(sibling);
    let sibling_hash = keccak256(&sibling);
    let third = Address::repeat_byte(3);
    let writer = factory.provider_rw().unwrap();
    writer
        .write_chunked_code(
            third,
            Account { bytecode_hash: Some(sibling_hash), ..Default::default() },
            &ValidatedCode::new(sibling.clone()).unwrap(),
        )
        .unwrap();
    writer.commit().unwrap();
    for replacement in [Bytes::from_static(&[0x60]), Bytes::new()] {
        let next = Account {
            bytecode_hash: (!replacement.is_empty()).then(|| keccak256(&replacement)),
            ..account.clone()
        };
        let writer = factory.provider_rw().unwrap();
        writer
            .write_chunked_code(
                owners[0],
                next.clone(),
                &ValidatedCode::new(replacement.clone()).unwrap(),
            )
            .unwrap();
        writer.commit().unwrap();
        let current = factory.latest().unwrap();
        assert_eq!(current.basic_account(&owners[0]).unwrap(), Some(next));
        // Resolve the identity from the surviving account rather than supplying its hash.
        let survivor = current.basic_account(&owners[1]).unwrap().unwrap();
        assert_eq!(survivor, account);
        let survivor_hash = survivor.bytecode_hash.unwrap();
        assert_eq!(
            current.bytecode_by_hash(&survivor_hash).unwrap().unwrap().original_bytes(),
            original
        );
        for index in 0..3 {
            assert_eq!(
                current.get_code_chunk_by_hash(&survivor_hash, index).unwrap().unwrap().as_ref(),
                &original
                    [index as usize * 24541..((index as usize + 1) * 24541).min(original.len())]
            );
        }
        let old = snapshot.basic_account(&owners[0]).unwrap().unwrap();
        assert_eq!(old, account);
        assert_eq!(
            snapshot
                .bytecode_by_hash(&old.bytecode_hash.unwrap())
                .unwrap()
                .unwrap()
                .original_bytes(),
            original
        );
        let sibling_account = current.basic_account(&third).unwrap().unwrap();
        assert_eq!(
            current
                .bytecode_by_hash(&sibling_account.bytecode_hash.unwrap())
                .unwrap()
                .unwrap()
                .original_bytes(),
            sibling
        );
        let reader = factory.provider().unwrap();
        assert!(reader.tx_ref().get::<tables::Bytecodes>(hash).unwrap().is_none());
        assert!(reader.tx_ref().get::<tables::BytecodeChunkDescriptors>(hash).unwrap().is_some());
    }
}

/// Conflicting claims cannot overwrite previously authenticated shared content.
#[test]
fn t019_conflicting_reingestion_preserves_committed_content() {
    let factory = create_test_provider_factory();
    let mut original = vec![0; 24542];
    original[0] = 0x60;
    original[1] = 7;
    let original = Bytes::from(original);
    let hash = keccak256(&original);
    let owner = Address::repeat_byte(0x19);
    let account = Account { bytecode_hash: Some(hash), ..Default::default() };
    let writer = factory.provider_rw().unwrap();
    writer
        .write_chunked_code(owner, account.clone(), &ValidatedCode::new(original.clone()).unwrap())
        .unwrap();
    writer.commit().unwrap();
    for repair_chunk_hash in [false, true] {
        let mut chunks = original.chunks(24541).map(Bytes::copy_from_slice).collect::<Vec<_>>();
        let mut hashes = chunks.iter().map(keccak256).collect::<Vec<_>>();
        let mut changed = chunks[0].to_vec();
        changed[1] = 8;
        chunks[0] = changed.into();
        let expected = if repair_chunk_hash {
            hashes[0] = keccak256(&chunks[0]);
            CodeValidationError::FullCodeHash
        } else {
            CodeValidationError::ChunkHash { index: 0 }
        };
        let writer = factory.provider_rw().unwrap();
        let before_descriptor =
            writer.tx_ref().get::<tables::BytecodeChunkDescriptors>(hash).unwrap();
        let before_payloads = writer.tx_ref().entries::<tables::BytecodeChunks>().unwrap();
        let error =
            writer.import_chunked_code(owner, account.clone(), 24542, hashes, chunks).unwrap_err();
        let ProviderError::InvalidChunkedCode(actual) = error else {
            panic!("unexpected error: {error:?}")
        };
        assert_eq!(actual, expected);
        assert_eq!(
            writer.tx_ref().get::<tables::BytecodeChunkDescriptors>(hash).unwrap(),
            before_descriptor
        );
        assert_eq!(writer.tx_ref().entries::<tables::BytecodeChunks>().unwrap(), before_payloads);
        assert_eq!(
            writer
                .tx_ref()
                .get::<tables::BytecodeChunks>(keccak256(&original[..24541]))
                .unwrap()
                .unwrap()
                .as_ref(),
            &original[..24541]
        );
        assert_eq!(
            writer.tx_ref().get::<tables::PlainAccountState>(owner).unwrap(),
            Some(account.clone())
        );
        writer.commit().unwrap();
        let state = factory.latest().unwrap();
        let persisted = state.basic_account(&owner).unwrap().unwrap();
        assert_eq!(persisted, account);
        assert_eq!(
            state
                .bytecode_by_hash(&persisted.bytecode_hash.unwrap())
                .unwrap()
                .unwrap()
                .original_bytes(),
            original
        );
    }
}

/// T-018: retained caches and snapshots resolve identity from their own account view.
#[test]
fn t018_latest_cached_and_snapshot_code_transitions() {
    let factory = create_test_provider_factory();
    let owner = Address::repeat_byte(0x18);
    let cache = ExecutionCache::new(4 * 1024 * 1024);
    let mut first = vec![0; 24542];
    first[0] = 0x60;
    first[1] = 0x11;
    let mut second = vec![0; 49083];
    second[0] = 0x60;
    second[1] = 0x22;
    second[49082] = 0x5b;
    // Includes empty-to-multi, multi-to-single, single-to-multi, multi-to-multi,
    // and multi-to-empty. Extensions are composed by the caller, not the writer.
    let states = [
        Bytes::new(),
        Bytes::from(first.clone()),
        Bytes::from_static(&[0x60]),
        Bytes::from(first),
        Bytes::from(second),
        Bytes::new(),
    ];
    let mut snapshots = Vec::new();
    for (step, original) in states.iter().enumerate() {
        let extension = if original.len() > 24541 {
            Bytes::from(vec![0xfe, 0x18, step as u8, 0xaa])
        } else {
            Bytes::from_static(&[0xfe, 0x18])
        };
        let account = Account {
            nonce: 19,
            balance: U256::from(1143),
            bytecode_hash: (!original.is_empty()).then(|| keccak256(original)),
            extension: extension.into(),
        };
        let writer = factory.provider_rw().unwrap();
        writer
            .write_chunked_code(
                owner,
                account.clone(),
                &ValidatedCode::new(original.clone()).unwrap(),
            )
            .unwrap();
        writer.commit().unwrap();
        let latest = factory.latest().unwrap();
        assert_account_code(&latest, owner, &account, original);
        // Cross-block caches require the same state-update handoff as production execution.
        // Retain immutable code entries while updating the account's current identity.
        let updates = BundleState::builder(1..=1)
            .state_present_account_info(
                owner,
                AccountInfo {
                    nonce: account.nonce,
                    balance: account.balance,
                    code_hash: account.bytecode_hash.unwrap_or_else(|| keccak256([])),
                    extension: Bytes::copy_from_slice(account.extension.as_ref()).into(),
                    ..Default::default()
                },
            )
            .build();
        cache.insert_state(&updates).unwrap();
        let cached = CachedStateProvider::new_prewarm(factory.latest().unwrap(), cache.clone());
        assert_account_code(&cached, owner, &account, original);
        assert_account_code(&cached, owner, &account, original);
        snapshots.push((latest, account, original.clone()));
        for (snapshot, old_account, old_bytes) in &snapshots {
            assert_account_code(snapshot, owner, old_account, old_bytes);
        }
    }
}

fn assert_account_code(
    state: &(impl AccountReader + BytecodeReader + CodeChunkReader),
    owner: Address,
    expected_account: &Account,
    original: &Bytes,
) {
    let account = state.basic_account(&owner).unwrap().unwrap();
    assert_eq!(&account, expected_account);
    // Derive the lookup identity from this provider's account, including empty code.
    let hash = account.bytecode_hash.unwrap_or_else(|| keccak256([]));
    if original.is_empty() {
        assert_eq!(state.get_code_chunk_by_hash(&hash, 0).unwrap(), None);
    } else {
        assert_eq!(state.bytecode_by_hash(&hash).unwrap().unwrap().original_bytes(), *original);
        for (index, expected) in original.chunks(24541).enumerate() {
            assert_eq!(
                state.get_code_chunk_by_hash(&hash, index as u32).unwrap(),
                Some(Bytes::copy_from_slice(expected))
            );
        }
    }
    let count = original.len().div_ceil(24541) as u32;
    assert_eq!(state.get_code_chunk_by_hash(&hash, count).unwrap(), None);
    assert_eq!(state.get_code_chunk_by_hash(&hash, u32::MAX).unwrap(), None);
}

/// T-018/T-019: resolve historical identities through actual indexed changesets.
#[test]
fn t018_t019_historical_overlay_resolves_each_transition_and_shared_owner() {
    let factory = create_test_provider_factory();
    let mut blocks = Vec::new();
    let mut parent_hash = Default::default();
    for number in 0..7 {
        let block = RecoveredBlock::new_unhashed(
            Block {
                header: Header { number, parent_hash, ..Default::default() },
                body: Default::default(),
            },
            vec![],
        );
        parent_hash = block.hash();
        blocks.push(block);
    }
    let owner = Address::repeat_byte(0x81);
    let survivor = Address::repeat_byte(0x82);
    let mut first = vec![0; 24542];
    first[0] = 0x60;
    first[1] = 0x31;
    let mut second = first.clone();
    second[1] = 0x32;
    let originals = [
        Bytes::new(),
        Bytes::from(first.clone()),
        Bytes::from_static(&[0x60]),
        Bytes::from(first),
        Bytes::from(second),
        Bytes::new(),
    ];
    let accounts = originals
        .iter()
        .enumerate()
        .map(|(i, bytes)| Account {
            nonce: 9,
            balance: U256::from(73),
            bytecode_hash: (!bytes.is_empty()).then(|| keccak256(bytes)),
            extension: Bytes::from(vec![0xfe, if bytes.len() > 24541 { i as u8 + 1 } else { 0 }])
                .into(),
        })
        .collect::<Vec<_>>();
    let writer = factory.provider_rw().unwrap();
    for block in &blocks {
        writer.insert_block(block).unwrap();
    }
    for (i, (bytes, account)) in originals.iter().zip(&accounts).enumerate() {
        let block = i as u64 + 1;
        writer
            .tx_ref()
            .put::<tables::AccountChangeSets>(
                block,
                AccountBeforeTx {
                    address: owner,
                    info: i.checked_sub(1).map(|previous| accounts[previous].clone()),
                },
            )
            .unwrap();
        writer
            .write_chunked_code(owner, account.clone(), &ValidatedCode::new(bytes.clone()).unwrap())
            .unwrap();
    }
    writer
        .tx_ref()
        .put::<tables::AccountsHistory>(
            ShardedKey { key: owner, highest_block_number: u64::MAX },
            BlockNumberList::new(1..=6).unwrap(),
        )
        .unwrap();
    // A second owner retains the first multi-chunk identity after the first clears it.
    writer
        .write_chunked_code(
            survivor,
            accounts[1].clone(),
            &ValidatedCode::new(originals[1].clone()).unwrap(),
        )
        .unwrap();
    writer.save_stage_checkpoint(StageId::Finish, StageCheckpoint::new(6)).unwrap();
    writer.commit().unwrap();
    assert_account_code(&factory.latest().unwrap(), owner, &accounts[5], &originals[5]);
    assert_account_code(&factory.latest().unwrap(), survivor, &accounts[1], &originals[1]);
    for i in 0..5 {
        let historical = OverlayStateProviderFactory::<_, EthPrimitives>::new(
            factory.clone(),
            OverlayManager::default().overlay_builder(blocks[i + 1].hash()),
        );
        let view = historical.database_provider_ro().unwrap();
        assert_account_code(&view, owner, &accounts[i], &originals[i]);
        let cached = CachedStateProvider::new_prewarm(view, ExecutionCache::new(4 * 1024 * 1024));
        assert_account_code(&cached, owner, &accounts[i], &originals[i]);
        assert_account_code(&cached, owner, &accounts[i], &originals[i]);
    }
    // Exercise the production unwind path; do not emulate reversion by writing
    // the expected account directly. Immutable code shared by another owner stays.
    let writer = factory.provider_rw().unwrap();
    writer.take_state_above(2).unwrap();
    assert_eq!(
        writer.tx_ref().get::<tables::PlainAccountState>(owner).unwrap(),
        Some(accounts[1].clone())
    );
    writer.commit().unwrap();
    assert_account_code(&factory.latest().unwrap(), owner, &accounts[1], &originals[1]);
    assert_account_code(&factory.latest().unwrap(), survivor, &accounts[1], &originals[1]);
}

/// T-018/T-021: appended production overlays override conflicting persisted accounts.
#[test]
fn t018_resident_overlay_precedence_and_sparse_reads() {
    let hooks = DatabaseTestHooks::default();
    let factory = create_test_provider_factory_with_db_hooks(hooks.clone());
    let owner = Address::repeat_byte(0x83);
    let base = RecoveredBlock::new_unhashed(Block::default(), vec![]);
    let mut persisted = vec![0; 24542];
    persisted[0] = 0x60;
    persisted[1] = 0x41;
    let persisted = Bytes::from(persisted);
    let old =
        Account { nonce: 8, bytecode_hash: Some(keccak256(&persisted)), ..Default::default() };
    let writer = factory.provider_rw().unwrap();
    writer.insert_block(&base).unwrap();
    writer.save_stage_checkpoint(StageId::Finish, StageCheckpoint::new(0)).unwrap();
    writer
        .write_chunked_code(owner, old.clone(), &ValidatedCode::new(persisted.clone()).unwrap())
        .unwrap();
    writer.commit().unwrap();
    // Positive control observes the lower-priority persisted payload before the overlay exists.
    hooks.clear_reads();
    factory.latest().unwrap().get_code_chunk_by_hash(&keccak256(&persisted), 0).unwrap().unwrap();
    assert_eq!(
        hooks.reads().iter().filter(|r| r.table == "BytecodeChunks" && r.value.is_some()).count(),
        1
    );
    let mut resident = vec![0; 49083];
    resident[0] = 0x60;
    resident[1] = 0x51;
    resident[24541] = 0x60;
    resident[24542] = 0x52;
    resident[49082] = 0x5b;
    for original in [Bytes::from(resident), Bytes::from_static(&[0x60]), Bytes::new()] {
        let hash = keccak256(&original);
        let account = Account {
            nonce: 8,
            bytecode_hash: (!original.is_empty()).then_some(hash),
            extension: Bytes::from_static(&[0xfe, 0x83]).into(),
            ..Default::default()
        };
        let info = AccountInfo {
            nonce: account.nonce,
            balance: account.balance,
            code_hash: hash,
            extension: Bytes::copy_from_slice(account.extension.as_ref()).into(),
            code: None,
            ..Default::default()
        };
        assert_eq!(info.extension.as_ref(), account.extension.as_ref());
        let mut bundle =
            BundleState::builder(1..=1).state_present_account_info(owner, info).build();
        if !original.is_empty() {
            // This models already validated execution output, not a database fetch.
            ValidatedCode::new(original.clone()).unwrap();
            bundle.contracts.insert(hash, RevmBytecode::new_legacy(original.clone()));
        }
        let block = RecoveredBlock::new_unhashed(
            Block {
                header: Header { number: 1, parent_hash: base.hash(), ..Default::default() },
                body: Default::default(),
            },
            vec![],
        );
        let executed = ExecutedBlock::<EthPrimitives>::new(
            Arc::new(block),
            Arc::new(BlockExecutionOutput::new(Default::default(), bundle)),
            Default::default(),
        );
        hooks.clear_reads();
        let overlay = OverlayStateProviderFactory::<_, EthPrimitives>::new(
            factory.clone(),
            OverlayManager::default().overlay_builder(base.hash()).with_appended_block(executed),
        );
        let view = overlay.database_provider_ro().unwrap();
        assert_account_code(&view, owner, &account, &original);
        let cached = CachedStateProvider::new_prewarm(view, ExecutionCache::new(4 * 1024 * 1024));
        assert_account_code(&cached, owner, &account, &original);
        assert!(!hooks
            .reads()
            .iter()
            .any(|r| r.table == "Bytecodes" || r.table == "BytecodeChunks"));
        // Dropping the appended view restores the durable identity without writing it back.
        drop(cached);
        assert_account_code(&factory.latest().unwrap(), owner, &old, &persisted);
    }
}

//! Revised overlap layout through native outcomes, normal persistence and cold reopen.

use alloy_primitives::{keccak256, Address, Bytes, TxKind, U256};
use evm2::{
    env::{BlockEnvExt, TxEnvExt},
    ethereum::{execute_initial_frame, prepare_initial_frame},
    evm::Db,
    interpreter::{GasTracker, InstrStop},
    registry::TxRegistry,
    BaseEvmTypes, Evm, ExecutionConfig, Precompiles, SpecId, Version,
};
use reth_chainspec::MAINNET;
use reth_db::{
    init_db, mdbx::DatabaseArguments, open_db, test_utils::DatabaseTestHooks, DatabaseEnv,
};
use reth_db_api::{
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_evm::{database::StateProviderDatabase, Database};
use reth_execution_types::{
    decode_code_metadata, native_bytecode, BlockState, EvmStateChangeSink,
    ExecutionAccountChangeRef, ExecutionAccountInfo, ExecutionOutcome,
};
use reth_provider::{
    providers::RocksDBBuilder,
    test_utils::{create_test_provider_factory_with_db_hooks, MockNodeTypesWithDB},
    ProviderFactory, StateProviderFactory, StaticFileProviderBuilder,
};
use reth_storage_api::{
    BytecodeReader, DBProvider, MetadataWriter, StateProvider, StateWriteConfig, StateWriter,
    StorageSettings, StorageSettingsCache, ValidatedCode,
};
use reth_trie::{HashedPostState, KeccakKeyHasher};
use revm::{database::OriginalValuesKnown, state::Bytecode};
use std::{path::Path, sync::Arc};

#[test]
fn native_outcome_publishes_prepared_chunks_and_reopens_both_storage_versions() {
    for settings in [StorageSettings::v1(), StorageSettings::v2()] {
        let directory = tempfile::tempdir().unwrap();
        let factory = open_factory(directory.path(), true);
        let setup = factory.provider_rw().unwrap();
        setup.write_storage_settings(settings).unwrap();
        setup.commit().unwrap();
        factory.set_storage_settings_cache(settings);
        let address = Address::repeat_byte(0x14);
        let target = 39 * 24541;
        let mut bytes = vec![0x00; 24541 * 40];
        bytes[..6].copy_from_slice(&[
            0x62,
            (target >> 16) as u8,
            (target >> 8) as u8,
            target as u8,
            0x56,
            0,
        ]);
        bytes[target - 1] = 0x7f;
        bytes[target..target + 32].fill(0xa7);
        bytes[target + 32..target + 41]
            .copy_from_slice(&[0x5b, 0x60, 42, 0x60, 0, 0x52, 0x60, 32, 0x5f]);
        bytes[target + 41] = 0xf3;
        let original = Bytes::from(bytes);
        let code = native_bytecode(&Bytecode::new_legacy(original.clone()));
        let validated = ValidatedCode::new(original.clone()).unwrap();
        let descriptor = validated.descriptor().unwrap();
        let mut encoded = vec![1];
        encoded.extend_from_slice(&(original.len() as u32).to_be_bytes());
        encoded.push(descriptor.chunk_hashes().len() as u8);
        for hash in descriptor.chunk_hashes() {
            encoded.extend_from_slice(hash.as_slice());
        }
        let metadata = decode_code_metadata(validated.code_hash(), &encoded).unwrap().0;
        let info = ExecutionAccountInfo {
            nonce: 1,
            code_hash: validated.code_hash(),
            code_metadata: metadata,
            extension: Bytes::from_static(b"unrelated fields").into(),
            ..Default::default()
        };
        let mut block = BlockState::new();
        let mut sink = block.transaction_sink();
        sink.bytecode(info.code_hash, &code).unwrap();
        sink.account(ExecutionAccountChangeRef {
            address,
            original: None,
            current: Some(&info),
            created: true,
            selfdestructed: false,
        })
        .unwrap();
        drop(sink);
        let outcome = ExecutionOutcome::new(block.into_bundle(), vec![vec![]], 1, vec![]);
        let writer = factory.provider_rw().unwrap();
        writer
            .write_state(
                &outcome,
                OriginalValuesKnown::Yes,
                StateWriteConfig {
                    write_receipts: false,
                    write_account_changesets: false,
                    write_storage_changesets: false,
                },
            )
            .unwrap();
        if settings.use_hashed_state() {
            writer
                .write_hashed_state(
                    &HashedPostState::from_bundle_state::<KeccakKeyHasher>(outcome.bundle.state())
                        .into_sorted(),
                )
                .unwrap();
        }
        writer.commit().unwrap();
        drop(factory);
        let hooks = DatabaseTestHooks::default();
        let factory = open_factory_observed(directory.path(), false, hooks.clone());
        factory.set_storage_settings_cache(settings);
        assert!(factory
            .provider()
            .unwrap()
            .tx_ref()
            .get::<tables::Bytecodes>(info.code_hash)
            .unwrap()
            .is_none());
        let state = factory.latest().unwrap();
        assert_eq!(
            state.bytecode_by_hash(&info.code_hash).unwrap().unwrap().original_bytes(),
            original
        );
        hooks.clear_reads();
        let mut execution = Evm::<'_, BaseEvmTypes>::new_with_execution_config(
            ExecutionConfig::for_spec_and_version(
                SpecId::PRAGUE,
                Version::new(SpecId::PRAGUE).with_tip1143(true),
            ),
            SpecId::PRAGUE,
            BlockEnvExt::default(),
            TxRegistry::new(),
            Db::new(StateProviderDatabase::new((&state).into_evm_state_provider())),
            Precompiles::base(SpecId::PRAGUE),
        );
        let budget = 200_000;
        let mut gas = GasTracker::new(budget);
        let frame = prepare_initial_frame(
            &mut execution,
            Address::repeat_byte(0x55),
            0,
            TxKind::Call(address),
            &Bytes::new(),
            U256::ZERO,
            &mut gas,
        )
        .unwrap();
        let result =
            execute_initial_frame(&mut execution, &TxEnvExt::default(), frame, &mut gas, budget, 0)
                .unwrap();
        assert_eq!(result.stop, InstrStop::Return);
        assert_eq!(result.output.len(), 32);
        assert_eq!(result.output[31], 42);
        let observed = hooks.reads();
        let payload_reads =
            observed.iter().filter(|read| read.table == "BytecodeChunks").collect::<Vec<_>>();
        assert_eq!(payload_reads.len(), 2);
        assert_eq!(payload_reads[0].key, descriptor.chunk_hashes()[0].as_slice());
        assert_eq!(payload_reads[1].key, descriptor.chunk_hashes()[39].as_slice());
        assert!(!observed.iter().any(|read| read.table == "Bytecodes"));
        drop(execution);
        let mut db = StateProviderDatabase::new(state.into_evm_state_provider());
        let loaded = db.get_account(&address).unwrap().unwrap();
        assert_eq!(loaded.code_metadata, info.code_metadata);
        assert_eq!(loaded.extension.as_ref(), b"unrelated fields");
        let first =
            Database::get_code_chunk_by_hash(&mut db, &info.code_hash, 38).unwrap().unwrap();
        let second =
            Database::get_code_chunk_by_hash(&mut db, &info.code_hash, 39).unwrap().unwrap();
        assert_eq!(first.prepared().unwrap().lookahead().as_ref(), &[0xa7; 32]);
        assert_eq!(second.prepared().unwrap().leading_data_len(), 32);
        assert_eq!(second.original_bytes().as_ref(), &original[target..target + 24541]);
        let execution = second.bytecode(true);
        for local in 0..=32 {
            assert!(execution.legacy_jump_table().unwrap().is_valid(local));
        }
        assert_eq!(keccak256(&original), info.code_hash);
    }
}

#[test]
fn full_code_reconstruction_rejects_context_tampering() {
    let directory = tempfile::tempdir().unwrap();
    let factory = open_factory(directory.path(), true);
    let mut bytes = vec![0; 24542];
    bytes[24540] = 0x60;
    bytes[24541] = 0xa7;
    let code = ValidatedCode::new(bytes.into()).unwrap();
    let writer = factory.provider_rw().unwrap();
    writer
        .write_chunked_code(
            Address::ZERO,
            reth_primitives_traits::Account {
                bytecode_hash: Some(code.code_hash()),
                ..Default::default()
            },
            &code,
        )
        .unwrap();
    let descriptor = code.descriptor().unwrap();
    // Same raw hashes and size, but wrong leading/spill preparation for this complete contract.
    let wrong = reth_storage_api::CodeChunkDescriptor::new(
        descriptor.code_size(),
        descriptor.chunk_hashes().to_vec(),
    )
    .unwrap();
    writer.tx_ref().put::<tables::BytecodeChunkDescriptors>(code.code_hash(), wrong).unwrap();
    writer.commit().unwrap();
    assert!(factory.latest().unwrap().bytecode_by_hash(&code.code_hash()).is_err());
}

fn open_factory(
    path: &Path,
    create: bool,
) -> ProviderFactory<MockNodeTypesWithDB<Arc<DatabaseEnv>>> {
    open_factory_observed(path, create, DatabaseTestHooks::default())
}

fn open_factory_observed(
    path: &Path,
    create: bool,
    hooks: DatabaseTestHooks,
) -> ProviderFactory<MockNodeTypesWithDB<Arc<DatabaseEnv>>> {
    let database = if create {
        init_db(path.join("db"), DatabaseArguments::default().with_test_hooks(hooks)).unwrap()
    } else {
        open_db(path.join("db"), DatabaseArguments::default().with_test_hooks(hooks)).unwrap()
    };
    ProviderFactory::new(
        Arc::new(database),
        MAINNET.clone(),
        StaticFileProviderBuilder::read_write(path.join("static_files")).build().unwrap(),
        RocksDBBuilder::new(path.join("rocksdb")).with_default_tables().build().unwrap(),
        reth_tasks::Runtime::test(),
    )
    .unwrap()
}

#[test]
fn inline_account_and_rpc_code_use_no_marker_payload_reads() {
    let hooks = DatabaseTestHooks::default();
    let factory = create_test_provider_factory_with_db_hooks(hooks.clone());
    let address = Address::repeat_byte(0x77);
    let target = Address::repeat_byte(0x42);
    let marker = Bytecode::new_eip7702(target);
    let info = ExecutionAccountInfo {
        nonce: 1,
        code_hash: marker.hash_slow(),
        inline_delegation: Some(target),
        extension: Bytes::from_static(b"opaque").into(),
        ..Default::default()
    };
    let mut block = BlockState::new();
    block
        .transaction_sink()
        .account(ExecutionAccountChangeRef {
            address,
            original: None,
            current: Some(&info),
            created: false,
            selfdestructed: false,
        })
        .unwrap();
    let outcome = ExecutionOutcome::new(block.into_bundle(), vec![vec![]], 1, vec![]);
    let writer = factory.provider_rw().unwrap();
    writer
        .write_state(
            &outcome,
            OriginalValuesKnown::Yes,
            StateWriteConfig {
                write_receipts: false,
                write_account_changesets: false,
                write_storage_changesets: false,
            },
        )
        .unwrap();
    writer.commit().unwrap();
    hooks.clear_reads();
    let state = factory.latest().unwrap();
    let mut db = StateProviderDatabase::new((&state).into_evm_state_provider());
    let loaded = db.get_account(&address).unwrap().unwrap();
    assert_eq!(loaded.inline_delegation, Some(target));
    assert_eq!(loaded.extension.as_ref(), b"opaque");
    assert_eq!(
        state.account_code(&address).unwrap().unwrap().original_bytes(),
        marker.original_bytes()
    );
    assert!(!hooks.reads().iter().any(|read| matches!(
        read.table.as_str(),
        "Bytecodes" | "BytecodeChunks" | "BytecodeChunkDescriptors"
    )));
}

#[test]
fn identical_payloads_retain_distinct_preparation_context() {
    let hooks = DatabaseTestHooks::default();
    let factory = create_test_provider_factory_with_db_hooks(hooks);
    let mut first = vec![0; 24541 * 2];
    first[24540] = 0x7f;
    first[24541..24573].fill(0xa7);
    let mut second = first.clone();
    second[24540] = 0;
    let first = ValidatedCode::new(first.into()).unwrap();
    let second = ValidatedCode::new(second.into()).unwrap();
    assert_eq!(
        first.descriptor().unwrap().chunk_hashes()[1],
        second.descriptor().unwrap().chunk_hashes()[1]
    );
    let writer = factory.provider_rw().unwrap();
    for (address, code) in [(Address::repeat_byte(1), &first), (Address::repeat_byte(2), &second)] {
        writer
            .write_chunked_code(
                address,
                reth_primitives_traits::Account {
                    bytecode_hash: Some(code.code_hash()),
                    ..Default::default()
                },
                code,
            )
            .unwrap();
    }
    writer.commit().unwrap();
    let state = factory.latest().unwrap();
    assert_eq!(
        state
            .code_chunk_descriptor(&first.code_hash())
            .unwrap()
            .unwrap()
            .preparation(1)
            .unwrap()
            .leading_data_len,
        32
    );
    assert_eq!(
        state
            .code_chunk_descriptor(&second.code_hash())
            .unwrap()
            .unwrap()
            .preparation(1)
            .unwrap()
            .leading_data_len,
        0
    );
}

#[test]
fn ordinary_account_metadata_does_not_fetch_code_and_kind_preserves_legacy_records() {
    for delegated in [false, true] {
        let hooks = DatabaseTestHooks::default();
        let factory = create_test_provider_factory_with_db_hooks(hooks.clone());
        let address = Address::repeat_byte(0x23);
        let target = Address::repeat_byte(0x34);
        let marker = Bytecode::new_eip7702(target);
        let code =
            if delegated { marker.clone() } else { Bytecode::new_legacy(marker.original_bytes()) };
        let hash = code.hash_slow();
        let writer = factory.provider_rw().unwrap();
        writer
            .tx_ref()
            .put::<tables::Bytecodes>(hash, reth_primitives_traits::Bytecode(code))
            .unwrap();
        writer
            .tx_ref()
            .put::<tables::PlainAccountState>(
                address,
                reth_primitives_traits::Account {
                    nonce: 1,
                    bytecode_hash: Some(hash),
                    ..Default::default()
                },
            )
            .unwrap();
        writer.commit().unwrap();
        let state = factory.latest().unwrap();
        let mut db = StateProviderDatabase::new(state.into_evm_state_provider());
        hooks.clear_reads();
        let loaded = db.get_account(&address).unwrap().unwrap();
        assert!(loaded.code.is_none());
        assert!(loaded.inline_delegation.is_none());
        assert!(!hooks.reads().iter().any(|read| matches!(
            read.table.as_str(),
            "Bytecodes" | "BytecodeChunks" | "BytecodeChunkDescriptors"
        )));
        let kind = db.get_code_kind_by_hash(&hash).unwrap();
        assert_eq!(
            kind,
            if delegated {
                evm2::bytecode::BytecodeKind::Eip7702
            } else {
                evm2::bytecode::BytecodeKind::Legacy
            }
        );
        assert_eq!(hooks.reads().iter().filter(|read| read.table == "Bytecodes").count(), 1);
        assert!(reth_execution_types::revm_account(&loaded).extension.code_metadata().is_none());
    }
}

#[test]
fn cold_reopen_executes_generated_and_split_rjump() {
    for suffix in [&[0x60, 0xab, 0x50, 0][..], &[0xe0, 0x80, 0x80, 0][..]] {
        let directory = tempfile::tempdir().unwrap();
        let factory = open_factory(directory.path(), true);
        let settings = StorageSettings::v1();
        factory.set_storage_settings_cache(settings);
        let writer = factory.provider_rw().unwrap();
        writer.write_storage_settings(settings).unwrap();
        let address = Address::repeat_byte(0x65);
        let mut raw = vec![0; 24540];
        // Jump to the JUMPDEST preceding the boundary instruction.
        raw[..4].copy_from_slice(&[0x61, 0x5f, 0xdb, 0x56]);
        raw[24539] = 0x5b;
        raw.extend_from_slice(suffix);
        let code = ValidatedCode::new(raw.into()).unwrap();
        writer
            .write_chunked_code(
                address,
                reth_primitives_traits::Account::from(reth_execution_types::revm_account(
                    &ExecutionAccountInfo {
                        nonce: 1,
                        code_hash: code.code_hash(),
                        code_metadata: evm2::bytecode::code_metadata(code.original_bytes())
                            .unwrap(),
                        ..Default::default()
                    },
                )),
                &code,
            )
            .unwrap();
        writer.commit().unwrap();
        drop(factory);
        let hooks = DatabaseTestHooks::default();
        let factory = open_factory_observed(directory.path(), false, hooks.clone());
        factory.set_storage_settings_cache(settings);
        let state = factory.latest().unwrap();
        let mut execution = Evm::<'_, BaseEvmTypes>::new_with_execution_config(
            ExecutionConfig::for_spec_and_version(
                SpecId::PRAGUE,
                Version::new(SpecId::PRAGUE).with_tip1143(true),
            ),
            SpecId::PRAGUE,
            BlockEnvExt::default(),
            TxRegistry::new(),
            Db::new(StateProviderDatabase::new(state.into_evm_state_provider())),
            Precompiles::base(SpecId::PRAGUE),
        );
        hooks.clear_reads();
        let budget = 100_000;
        let mut gas = GasTracker::new(budget);
        let frame = prepare_initial_frame(
            &mut execution,
            Address::repeat_byte(0x55),
            0,
            TxKind::Call(address),
            &Bytes::new(),
            U256::ZERO,
            &mut gas,
        )
        .unwrap();
        let result =
            execute_initial_frame(&mut execution, &TxEnvExt::default(), frame, &mut gas, budget, 0)
                .unwrap();
        assert_eq!(result.stop, InstrStop::Stop);
        let ordinary_gas = if suffix[0] == 0x60 { 19 } else { 14 };
        assert_eq!(gas.spent(), 2 * 28680 + ordinary_gas);
        let reads = hooks.reads();
        let payloads =
            reads.iter().filter(|read| read.table == "BytecodeChunks").collect::<Vec<_>>();
        assert_eq!(payloads.len(), 2);
        let hashes = code.descriptor().unwrap().chunk_hashes();
        assert_eq!(payloads[0].key, hashes[0].as_slice());
        assert_eq!(payloads[1].key, hashes[1].as_slice());
        assert!(!reads.iter().any(|read| read.table == "Bytecodes"));
    }
}

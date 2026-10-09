//! T-006 and T-030: original payloads are durable, including a one-byte STOP.
//!
//! Proposed production writer: DatabaseProviderRW::write_chunked_code(address, account,
//! &ValidatedCode). The supplied account is caller-composed; the writer validates its hash.
//! Proposed reader: CodeChunkReader::get_code_chunk_by_hash(&B256, u32) ->
//! ProviderResult<Option<Bytes>>. Descriptors use BytecodeChunkDescriptors; original
//! payloads use BytecodeChunks keyed by their Keccak hashes.

use alloy_primitives::{keccak256, Address, Bytes};
use reth_chainspec::MAINNET;
use reth_db::{
    init_db, mdbx::DatabaseArguments, open_db, test_utils::DatabaseTestHooks, Database, DatabaseEnv,
};
use reth_db_api::{tables, transaction::DbTx};
use reth_primitives_traits::Account;
use reth_provider::{
    providers::RocksDBBuilder,
    test_utils::{create_test_provider_factory_with_db_hooks, MockNodeTypesWithDB},
    ProviderFactory, StateProviderFactory, StaticFileProviderBuilder,
};
use reth_storage_api::{AccountReader, BytecodeReader, CodeChunkReader, DBProvider, ValidatedCode};
use std::{path::Path, sync::Arc};

#[test]
fn t006_persistent_chunks_without_whole_code_row() {
    for size in [24542, 957100, 981640] {
        let directory = tempfile::tempdir().unwrap();
        let factory = open_factory(directory.path(), true);
        let original = fixture(size);
        let hash = keccak256(&original);
        let address = Address::repeat_byte(0x43);
        let account = Account { nonce: 7, bytecode_hash: Some(hash), ..Default::default() };
        {
            let code = ValidatedCode::new(original.clone()).unwrap();
            let writer = factory.provider_rw().unwrap();
            writer.write_chunked_code(address, account.clone(), &code).unwrap();
            writer.commit().unwrap();
        }
        drop(factory);
        let factory = open_factory(directory.path(), false);
        let tx = factory.db_ref().tx().unwrap();
        assert!(tx.get::<tables::Bytecodes>(hash).unwrap().is_none());
        let descriptor = tx.get::<tables::BytecodeChunkDescriptors>(hash).unwrap().unwrap();
        assert_eq!(descriptor.code_size(), size as u32);
        for (index, expected) in original.chunks(24541).enumerate() {
            assert_eq!(descriptor.chunk_hashes()[index], keccak256(expected));
            assert_eq!(
                tx.get::<tables::BytecodeChunks>(keccak256(expected)).unwrap().unwrap().as_ref(),
                expected
            );
        }
        let state = factory.latest().unwrap();
        assert_eq!(state.basic_account(&address).unwrap(), Some(account));
        for (index, expected) in original.chunks(24541).enumerate() {
            assert_eq!(
                state.get_code_chunk_by_hash(&hash, index as u32).unwrap().unwrap().as_ref(),
                expected
            );
        }
        assert_eq!(state.bytecode_by_hash(&hash).unwrap().unwrap().original_bytes(), original);
    }
}

#[test]
fn t030_final_zero_is_nonempty_original_content() {
    for last in [0x00, 0x01] {
        let directory = tempfile::tempdir().unwrap();
        let factory = open_factory(directory.path(), true);
        let mut bytes = vec![0; 24542];
        bytes[24541] = last;
        let original = Bytes::from(bytes);
        let hash = keccak256(&original);
        let writer = factory.provider_rw().unwrap();
        writer
            .write_chunked_code(
                Address::repeat_byte(last + 1),
                Account { bytecode_hash: Some(hash), ..Default::default() },
                &ValidatedCode::new(original.clone()).unwrap(),
            )
            .unwrap();
        writer.commit().unwrap();
        drop(factory);
        let factory = open_factory(directory.path(), false);
        let raw = factory.provider().unwrap();
        let descriptor =
            raw.tx_ref().get::<tables::BytecodeChunkDescriptors>(hash).unwrap().unwrap();
        assert_eq!(descriptor.code_size(), 24542);
        assert_eq!(descriptor.chunk_hashes(), &[keccak256(&original[..24541]), keccak256([last])]);
        assert_eq!(
            raw.tx_ref().get::<tables::BytecodeChunks>(keccak256([last])).unwrap(),
            Some(Bytes::from(vec![last]))
        );
        assert!(raw.tx_ref().get::<tables::Bytecodes>(hash).unwrap().is_none());
        let state = factory.latest().unwrap();
        assert_eq!(state.get_code_chunk_by_hash(&hash, 1).unwrap(), Some(Bytes::from(vec![last])));
        assert_eq!(state.get_code_chunk_by_hash(&hash, 2).unwrap(), None);
        assert_eq!(state.get_code_chunk_by_hash(&keccak256([]), 0).unwrap(), None);
        assert_eq!(state.bytecode_by_hash(&hash).unwrap().unwrap().original_bytes(), original);
    }
}

fn fixture(size: usize) -> Bytes {
    let mut bytes = vec![0; size];
    for (index, chunk) in bytes.chunks_mut(24541).enumerate() {
        // PUSH1 tag; STOP fills the remainder. The one-byte final chunk is STOP.
        if chunk.len() >= 2 {
            chunk[0] = 0x60;
            chunk[1] = index as u8;
        }
    }
    bytes.into()
}

fn open_factory(
    path: &Path,
    create: bool,
) -> ProviderFactory<MockNodeTypesWithDB<Arc<DatabaseEnv>>> {
    let database = if create {
        init_db(path.join("db"), DatabaseArguments::default()).unwrap()
    } else {
        open_db(path.join("db"), DatabaseArguments::default()).unwrap()
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

/// T-030: canonical empty identity must not consult a payload table.
#[test]
fn t030_empty_identity_skips_storage_with_positive_control() {
    let hooks = DatabaseTestHooks::default();
    let factory = create_test_provider_factory_with_db_hooks(hooks.clone());
    let original = Bytes::from(vec![0; 24542]);
    let hash = keccak256(&original);
    let writer = factory.provider_rw().unwrap();
    writer
        .write_chunked_code(
            Address::repeat_byte(0x30),
            Account { bytecode_hash: Some(hash), ..Default::default() },
            &ValidatedCode::new(original).unwrap(),
        )
        .unwrap();
    writer.commit().unwrap();
    hooks.clear_reads();
    let state = factory.latest().unwrap();
    assert_eq!(state.get_code_chunk_by_hash(&hash, 1).unwrap(), Some(Bytes::from_static(&[0])));
    assert_eq!(hooks.reads().iter().filter(|r| r.table == "BytecodeChunks").count(), 1);
    assert_eq!(
        hooks.reads().iter().find(|r| r.table == "BytecodeChunks").unwrap().value.as_deref(),
        Some(&[0][..])
    );
    drop(state);
    hooks.clear_reads();
    let empty_state = factory.latest().unwrap();
    for index in [0, 1, u32::MAX] {
        assert_eq!(empty_state.get_code_chunk_by_hash(&keccak256([]), index).unwrap(), None);
    }
    assert!(!hooks.reads().iter().any(|r| matches!(
        r.table.as_str(),
        "Bytecodes" | "BytecodeChunks" | "BytecodeChunkDescriptors"
    )));
}

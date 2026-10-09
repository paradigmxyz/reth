//! T-007: observe MDBX operations from before provider construction.
//!
//! Proposed test-utils seam: DatabaseTestHooks records successful reads from get,
//! get_by_encoded_key, and every cursor route, with table, encoded key, and value.
//! create_test_provider_factory_with_db_hooks installs it before factory construction.
//! The hook delegates to real MDBX; it must never supply data or implement chunk logic.

use alloy_primitives::{keccak256, Address, Bytes};
use reth_chainspec::MAINNET;
use reth_db::{
    init_db, mdbx::DatabaseArguments, open_db, test_utils::DatabaseTestHooks, DatabaseEnv,
};
use reth_db_api::{
    cursor::DbCursorRO,
    table::Encode,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives_traits::{Account, Bytecode};
use reth_provider::{
    providers::RocksDBBuilder, test_utils::MockNodeTypesWithDB, ProviderFactory,
    StateProviderFactory, StaticFileProviderBuilder,
};
use reth_storage_api::{CodeChunkReader, DBProvider, ValidatedCode};
use std::{path::Path, sync::Arc};

#[test]
fn t007_only_selected_original_payload_is_read() {
    let hooks = DatabaseTestHooks::default();
    let directory = tempfile::tempdir().unwrap();
    let factory = open_observed_factory(directory.path(), true, hooks.clone());
    let mut original = vec![0; 957100];
    for (index, chunk) in original.chunks_mut(24541).enumerate() {
        if chunk.len() > 1 {
            chunk[0] = 0x60;
            chunk[1] = index as u8;
        } else {
            chunk[0] = 0x01;
        }
    }
    let hash = keccak256(&original);
    let writer = factory.provider_rw().unwrap();
    writer
        .write_chunked_code(
            Address::ZERO,
            Account { bytecode_hash: Some(hash), ..Default::default() },
            &ValidatedCode::new(original.clone().into()).unwrap(),
        )
        .unwrap();
    writer
        .tx_ref()
        .put::<tables::Bytecodes>(hash, Bytecode::new_raw(original.clone().into()))
        .unwrap();
    writer.commit().unwrap();
    // Independent positive controls show payload, unrelated payload, and full-code visibility.
    hooks.clear_reads();
    let reader = factory.provider().unwrap();
    for index in [0, 17] {
        reader
            .tx_ref()
            .get::<tables::BytecodeChunks>(keccak256(&original[index * 24541..(index + 1) * 24541]))
            .unwrap()
            .unwrap();
    }
    reader.tx_ref().get::<tables::Bytecodes>(hash).unwrap().unwrap();
    let controls = hooks.reads();
    assert_eq!(
        controls.iter().filter(|r| r.table == "BytecodeChunks" && r.value.is_some()).count(),
        2
    );
    assert_eq!(controls.iter().filter(|r| r.table == "Bytecodes" && r.value.is_some()).count(), 1);
    // The observer must also see encoded-key and cursor reads, not just DbTx::get.
    let selected = keccak256(&original[17 * 24541..18 * 24541]);
    hooks.clear_reads();
    reader
        .tx_ref()
        .get_by_encoded_key::<tables::BytecodeChunks>(&selected.encode())
        .unwrap()
        .unwrap();
    let encoded_reads = hooks.reads();
    assert_eq!(encoded_reads.iter().filter(|r| r.table == "BytecodeChunks").count(), 1);
    assert_eq!(
        encoded_reads.iter().find(|r| r.table == "BytecodeChunks").unwrap().key,
        selected.as_slice()
    );
    hooks.clear_reads();
    let mut cursor = reader.tx_ref().cursor_read::<tables::BytecodeChunks>().unwrap();
    let (key, value) = cursor.seek_exact(selected).unwrap().unwrap();
    assert_eq!(key, selected);
    assert_eq!(value.as_ref(), &original[17 * 24541..18 * 24541]);
    let cursor_reads = hooks.reads();
    assert_eq!(cursor_reads.iter().filter(|r| r.table == "BytecodeChunks").count(), 1);
    let observed = cursor_reads.iter().find(|r| r.table == "BytecodeChunks").unwrap();
    assert_eq!(observed.key, selected.as_slice());
    assert_eq!(observed.value.as_deref(), Some(&original[17 * 24541..18 * 24541]));
    drop(cursor);
    drop(reader);
    drop(factory);
    for index in [0, 17, 39, 40, u32::MAX] {
        hooks.clear_reads();
        // Keep every observation from environment/factory initialization through the request.
        let factory = open_observed_factory(directory.path(), false, hooks.clone());
        let state = factory.latest().unwrap();
        let actual = state.get_code_chunk_by_hash(&hash, index).unwrap();
        let reads = hooks.reads();
        assert!(!reads.iter().any(|r| r.table == "Bytecodes"));
        let payloads = reads.iter().filter(|r| r.table == "BytecodeChunks").collect::<Vec<_>>();
        if index < 40 {
            let start = index as usize * 24541;
            let expected = &original[start..(start + 24541).min(original.len())];
            assert_eq!(actual, Some(Bytes::copy_from_slice(expected)));
            assert_eq!(payloads.len(), 1);
            assert_eq!(payloads[0].key, keccak256(expected).as_slice());
            assert_eq!(payloads[0].value.as_deref(), Some(expected));
        } else {
            assert_eq!(actual, None);
            assert!(payloads.is_empty());
        }
        for metadata in reads.iter().filter(|r| r.table == "BytecodeChunkDescriptors") {
            assert!(metadata
                .value
                .as_ref()
                .is_none_or(|value| value.len() <= 5 + 40 * (32 + 4 + 32)));
        }
    }
    let factory = open_observed_factory(directory.path(), false, hooks.clone());
    let writer = factory.provider_rw().unwrap();
    assert!(writer
        .tx_ref()
        .delete::<tables::BytecodeChunks>(keccak256(&original[..24541]), None)
        .unwrap());
    writer.commit().unwrap();
    drop(factory);
    hooks.clear_reads();
    let factory = open_observed_factory(directory.path(), false, hooks.clone());
    let state = factory.latest().unwrap();
    assert_eq!(
        state.get_code_chunk_by_hash(&hash, 17).unwrap().unwrap().as_ref(),
        &original[17 * 24541..18 * 24541]
    );
    assert_eq!(hooks.reads().iter().filter(|r| r.table == "BytecodeChunks").count(), 1);
}

/// Install the observation seam before opening storage, never after constructing caches.
fn open_observed_factory(
    path: &Path,
    create: bool,
    hooks: DatabaseTestHooks,
) -> ProviderFactory<MockNodeTypesWithDB<Arc<DatabaseEnv>>> {
    let arguments = DatabaseArguments::default().with_test_hooks(hooks);
    let database = if create {
        init_db(path.join("db"), arguments).unwrap()
    } else {
        open_db(path.join("db"), arguments).unwrap()
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

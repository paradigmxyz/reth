//! T-010: untrusted ingestion validates before transaction-visible mutation.
//!
//! Proposed import_chunked_code(address, account, size, hashes, payloads) accepts
//! untrusted owned values. Its error is ProviderError::InvalidChunkedCode with a
//! CodeValidationError. This is separate from write_chunked_code's validated input.

use alloy_primitives::{keccak256, Address, Bytes, U256};
use reth_chainspec::MAINNET;
use reth_db::{init_db, mdbx::DatabaseArguments, open_db, DatabaseEnv};
use reth_db_api::{tables, transaction::DbTx};
use reth_primitives_traits::Account;
use reth_provider::{
    providers::RocksDBBuilder,
    test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
    ProviderError, ProviderFactory, StateProviderFactory, StaticFileProviderBuilder,
};
use reth_storage_api::{
    AccountReader, BytecodeReader, CodeChunkReader, CodeValidationError, DBProvider, ValidatedCode,
};

use std::{path::Path, sync::Arc};

#[test]
fn t010_invalid_import_leaves_no_transaction_visible_writes() {
    let mut original = vec![0; 49089];
    original[0] = 0x60;
    original[1] = 1;
    original[24541] = 0x60;
    original[24542] = 2;
    let payloads = original.chunks(24541).map(Bytes::copy_from_slice).collect::<Vec<_>>();
    let hashes = payloads.iter().map(keccak256).collect::<Vec<_>>();
    let hash = keccak256(&original);
    let mut cases = Vec::new();
    for index in 0..3 {
        for length in [0, payloads[index].len() - 1, payloads[index].len() + 1] {
            let mut bad = payloads.clone();
            bad[index] = Bytes::from(vec![0; length]);
            cases.push((
                original.len() as u32,
                hashes.clone(),
                bad,
                CodeValidationError::ChunkLength {
                    index: index as u32,
                    expected: payloads[index].len(),
                    actual: length,
                },
            ));
        }
        let mut bad = payloads.clone();
        let mut bytes = bad[index].to_vec();
        bytes[1] ^= 0x01;
        bad[index] = bytes.into();
        cases.push((
            original.len() as u32,
            hashes.clone(),
            bad,
            CodeValidationError::ChunkHash { index: index as u32 },
        ));
    }
    let mut reordered = payloads.clone();
    reordered.swap(0, 1);
    cases.push((
        original.len() as u32,
        hashes.clone(),
        reordered.clone(),
        CodeValidationError::ChunkHash { index: 0 },
    ));
    let mut reordered_hashes = hashes.clone();
    reordered_hashes.swap(0, 1);
    cases.push((
        original.len() as u32,
        reordered_hashes,
        reordered,
        CodeValidationError::FullCodeHash,
    ));
    cases.push((
        981641,
        hashes.clone(),
        payloads.clone(),
        CodeValidationError::InvalidCodeSize { size: 981641 },
    ));
    // Isolate count validation from payload authentication.
    for count in [0, 2, 4, 41] {
        cases.push((
            original.len() as u32,
            vec![hashes[0]; count],
            payloads.clone(),
            CodeValidationError::HashCount { expected: 3, actual: count },
        ));
    }
    for count in [0, 2, 4] {
        let mut chunks = payloads.clone();
        chunks.resize(count, payloads[0].clone());
        cases.push((
            original.len() as u32,
            hashes.clone(),
            chunks,
            CodeValidationError::PayloadCount { expected: 3, actual: count },
        ));
    }
    for (size, hashes, chunks, expected) in cases {
        for replace in [false, true] {
            let factory = create_test_provider_factory();
            let old_bytes = Bytes::from(vec![0; 24542]);
            let old_hash = keccak256(&old_bytes);
            let old_account = Account {
                nonce: 17,
                balance: U256::from(99),
                bytecode_hash: Some(old_hash),
                extension: Bytes::from_static(&[0xff, 0x43]).into(),
            };
            if replace {
                let seed = factory.provider_rw().unwrap();
                seed.write_chunked_code(
                    Address::ZERO,
                    old_account.clone(),
                    &ValidatedCode::new(old_bytes.clone()).unwrap(),
                )
                .unwrap();
                seed.commit().unwrap();
            }

            let writer = factory.provider_rw().unwrap();
            let error = writer
                .import_chunked_code(
                    Address::ZERO,
                    Account { bytecode_hash: Some(hash), ..Default::default() },
                    size,
                    hashes.clone(),
                    chunks.clone(),
                )
                .unwrap_err();
            let ProviderError::InvalidChunkedCode(actual) = error else {
                panic!("unexpected error: {error:?}")
            };
            assert_eq!(actual, expected);
            // Inspect before cleanup or abort can conceal any mutation.
            assert_eq!(
                writer.tx_ref().entries::<tables::BytecodeChunkDescriptors>().unwrap(),
                usize::from(replace)
            );
            assert_eq!(
                writer.tx_ref().entries::<tables::BytecodeChunks>().unwrap(),
                if replace { 2 } else { 0 }
            );
            assert_eq!(
                writer.tx_ref().get::<tables::PlainAccountState>(Address::ZERO).unwrap(),
                replace.then_some(old_account.clone())
            );
            writer.commit().unwrap();
            let reader = factory.provider().unwrap();
            assert_eq!(
                reader.tx_ref().entries::<tables::BytecodeChunkDescriptors>().unwrap(),
                usize::from(replace)
            );
            assert_eq!(
                reader.tx_ref().entries::<tables::BytecodeChunks>().unwrap(),
                if replace { 2 } else { 0 }
            );
            let state = factory.latest().unwrap();
            assert_eq!(
                state.basic_account(&Address::ZERO).unwrap(),
                replace.then_some(old_account)
            );
            assert_eq!(state.bytecode_by_hash(&hash).unwrap(), None);
            if replace {
                assert_eq!(
                    state.bytecode_by_hash(&old_hash).unwrap().unwrap().original_bytes(),
                    old_bytes
                );
            }
        }
    }
    let factory = create_test_provider_factory();
    let writer = factory.provider_rw().unwrap();
    writer
        .import_chunked_code(
            Address::ZERO,
            Account { bytecode_hash: Some(hash), ..Default::default() },
            original.len() as u32,
            hashes,
            payloads,
        )
        .unwrap();
    writer.commit().unwrap();
    assert_eq!(
        factory.provider().unwrap().tx_ref().entries::<tables::BytecodeChunkDescriptors>().unwrap(),
        1
    );
}

/// Already compiled bytecode no longer needs author-provided boundary STOPs.
#[test]
fn t010_authenticated_crossing_instructions_prepare_successfully() {
    for (offset, opcode) in [(24540, 0x60), (24540, 0x01), (24541, 0x7f)] {
        let factory = create_test_provider_factory();
        let mut original = vec![0; 24542];
        original[offset] = opcode;
        let hash = keccak256(&original);
        let chunks = original.chunks(24541).map(Bytes::copy_from_slice).collect::<Vec<_>>();
        let hashes = chunks.iter().map(keccak256).collect::<Vec<_>>();
        let writer = factory.provider_rw().unwrap();
        writer
            .import_chunked_code(
                Address::ZERO,
                Account { bytecode_hash: Some(hash), ..Default::default() },
                24542,
                hashes,
                chunks,
            )
            .unwrap();
        writer.commit().unwrap();
        let state = factory.latest().unwrap();
        assert_eq!(
            state.bytecode_by_hash(&hash).unwrap().unwrap().original_bytes().as_ref(),
            original
        );
    }
}

/// T-010: validated bytes do not authenticate the caller-supplied account identity.
#[test]
fn t010_validated_writer_rejects_disagreeing_account_hash_before_mutation() {
    for size in [0, 1, 24541, 24542, 981640] {
        let factory = create_test_provider_factory();
        let address = Address::repeat_byte(0x10);
        let old_bytes = Bytes::from_static(&[0x60]);
        let old_hash = keccak256(&old_bytes);
        let old_account = Account {
            nonce: 81,
            balance: U256::from(1143),
            bytecode_hash: Some(old_hash),
            extension: Bytes::from_static(&[0xfe, 0x10]).into(),
        };
        let writer = factory.provider_rw().unwrap();
        writer
            .write_chunked_code(
                address,
                old_account.clone(),
                &ValidatedCode::new(old_bytes.clone()).unwrap(),
            )
            .unwrap();
        writer.commit().unwrap();
        let original = Bytes::from(vec![0; size]);
        let hash = keccak256(&original);
        assert_ne!(hash, old_hash);
        let code = ValidatedCode::new(original).unwrap();
        let writer = factory.provider_rw().unwrap();
        let error = writer.write_chunked_code(address, old_account.clone(), &code).unwrap_err();
        let ProviderError::InvalidChunkedCode(actual) = error else {
            panic!("unexpected account identity error: {error:?}")
        };
        assert_eq!(actual, CodeValidationError::FullCodeHash);
        assert_eq!(
            writer.tx_ref().get::<tables::PlainAccountState>(address).unwrap(),
            Some(old_account.clone())
        );
        assert_eq!(writer.tx_ref().entries::<tables::BytecodeChunkDescriptors>().unwrap(), 0);
        assert_eq!(writer.tx_ref().entries::<tables::BytecodeChunks>().unwrap(), 0);
        assert_eq!(writer.tx_ref().entries::<tables::Bytecodes>().unwrap(), 1);
        writer.commit().unwrap();
        let state = factory.latest().unwrap();
        assert_eq!(state.basic_account(&address).unwrap(), Some(old_account));
        assert_eq!(state.bytecode_by_hash(&old_hash).unwrap().unwrap().original_bytes(), old_bytes);
        let raw = factory.provider().unwrap();
        assert_eq!(raw.tx_ref().get::<tables::Bytecodes>(hash).unwrap(), None);
        assert_eq!(raw.tx_ref().entries::<tables::BytecodeChunkDescriptors>().unwrap(), 0);
        assert_eq!(raw.tx_ref().entries::<tables::BytecodeChunks>().unwrap(), 0);
    }
}

/// T-010: independently committed EF01 input is valid ordinary storage content.
#[test]
fn t010_ef01_legacy_import_preserves_original_payloads() {
    let directory = tempfile::tempdir().unwrap();
    let factory = open_factory(directory.path(), true);
    let mut original = vec![0; 24542];
    original[..2].copy_from_slice(&[0xef, 0x01]);
    let original = Bytes::from(original);
    let hash = keccak256(&original);
    let payloads = original.chunks(24541).map(Bytes::copy_from_slice).collect::<Vec<_>>();
    let hashes = payloads.iter().map(keccak256).collect::<Vec<_>>();
    let account = Account { bytecode_hash: Some(hash), ..Default::default() };
    let writer = factory.provider_rw().unwrap();
    writer
        .import_chunked_code(
            Address::ZERO,
            account.clone(),
            24542,
            hashes.clone(),
            payloads.clone(),
        )
        .unwrap();
    assert!(writer.tx_ref().get::<tables::Bytecodes>(hash).unwrap().is_none());
    assert_eq!(
        writer
            .tx_ref()
            .get::<tables::BytecodeChunkDescriptors>(hash)
            .unwrap()
            .unwrap()
            .chunk_hashes(),
        hashes
    );
    writer.commit().unwrap();
    drop(factory);
    let factory = open_factory(directory.path(), false);
    let state = factory.latest().unwrap();
    assert_eq!(state.basic_account(&Address::ZERO).unwrap(), Some(account));
    for (index, payload) in payloads.iter().enumerate() {
        assert_eq!(
            state.get_code_chunk_by_hash(&hash, index as u32).unwrap().as_ref(),
            Some(payload)
        );
    }
    let reconstructed = state.bytecode_by_hash(&hash).unwrap().unwrap();
    assert!(reconstructed.0.is_legacy());
    assert_eq!(reconstructed.original_bytes(), original);
    assert_eq!(keccak256(reconstructed.original_bytes()), hash);
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

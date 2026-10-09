//! T-011, T-012, T-013, T-029: required data cannot disappear into successful absence.
//!
//! Proposed handoff: CodeRepresentation::{Empty, Legacy, Chunked(descriptor)} and
//! CodeChunkReader::get_required_code_chunk(&hash, &representation, index).
//! Proposed errors are ProviderError::CodeChunk(CodeChunkError), with public hash,
//! index, optional expected_length, and CodeChunkErrorKind fields.

use alloy_primitives::{keccak256, Address, Bytes, B256};
use reth_chainspec::MAINNET;
use reth_db::{
    init_db, mdbx::DatabaseArguments, open_db, test_utils::DatabaseTestHooks, DatabaseEnv,
};
use reth_db_api::{
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives_traits::{Account, Bytecode};
use reth_provider::{
    providers::RocksDBBuilder,
    test_utils::{
        create_test_provider_factory, create_test_provider_factory_with_db_hooks,
        MockNodeTypesWithDB,
    },
    ProviderError, ProviderFactory, StateProviderFactory, StaticFileProviderBuilder,
};
use reth_storage_api::{
    AccountReader, BytecodeReader, CodeChunkDescriptor, CodeChunkReader, CodeRepresentation,
    DBProvider, ValidatedCode,
};
use reth_storage_errors::provider::{CodeChunkError, CodeChunkErrorKind};

use std::{path::Path, sync::Arc};

#[test]
fn t011_missing_and_wrong_length_chunks_never_use_full_code() {
    for index in 0..3usize {
        for replacement in [
            None,
            Some(0),
            Some(if index == 2 { 6 } else { 24540 }),
            Some(if index == 2 { 8 } else { 24542 }),
        ] {
            let hooks = DatabaseTestHooks::default();
            let factory = create_test_provider_factory_with_db_hooks(hooks.clone());
            let original = fixture();
            let hash = keccak256(&original);
            let code = ValidatedCode::new(original.clone()).unwrap();
            let writer = factory.provider_rw().unwrap();
            writer.write_chunked_code(Address::ZERO, account(hash), &code).unwrap();
            writer.commit().unwrap();
            let key =
                keccak256(&original[index * 24541..((index + 1) * 24541).min(original.len())]);
            let corrupt = factory.provider_rw().unwrap();
            corrupt
                .tx_ref()
                .put::<tables::Bytecodes>(hash, Bytecode::new_raw(original.clone()))
                .unwrap();
            if let Some(size) = replacement {
                corrupt
                    .tx_ref()
                    .put::<tables::BytecodeChunks>(key, Bytes::from(vec![0; size]))
                    .unwrap();
            } else {
                assert!(corrupt.tx_ref().delete::<tables::BytecodeChunks>(key, None).unwrap());
            }
            corrupt.commit().unwrap();
            // Positive control: the observer must see the decoy through the actual DB route.
            hooks.clear_reads();
            let reader = factory.provider().unwrap();
            assert!(reader.tx_ref().get::<tables::Bytecodes>(hash).unwrap().is_some());
            assert_eq!(hooks.reads().iter().filter(|read| read.table == "Bytecodes").count(), 1);
            drop(reader);
            hooks.clear_reads();
            let state = factory.latest().unwrap();
            let expected_length = if index == 2 { 7 } else { 24541 };
            let reason = replacement.map_or(CodeChunkErrorKind::MissingPayload, |actual| {
                CodeChunkErrorKind::InvalidLength { actual }
            });
            assert_chunk_error(
                state.get_code_chunk_by_hash(&hash, index as u32).unwrap_err(),
                ProviderError::CodeChunk(CodeChunkError {
                    context: None,
                    code_hash: hash,
                    index: index as u32,
                    expected_length: Some(expected_length),
                    reason,
                }),
            );
            assert!(!hooks.reads().iter().any(|read| read.table == "Bytecodes"));
            let attempted = hooks.reads();
            let payload_reads =
                attempted.iter().filter(|read| read.table == "BytecodeChunks").collect::<Vec<_>>();
            assert_eq!(payload_reads.len(), 1);
            assert_eq!(payload_reads[0].key, key.as_slice());
            let intact = (index + 1) % 3;
            assert_eq!(
                state.get_code_chunk_by_hash(&hash, intact as u32).unwrap().unwrap().as_ref(),
                &original[intact * 24541..((intact + 1) * 24541).min(original.len())]
            );
            assert!(state.bytecode_by_hash(&hash).is_err());
        }
    }
}

#[test]
fn t012_committed_descriptor_missing_or_disagreeing_is_an_error() {
    let factory = create_test_provider_factory();
    let original = fixture();
    let hash = keccak256(&original);
    let code = ValidatedCode::new(original.clone()).unwrap();
    let committed = code.descriptor().unwrap().clone();
    let writer = factory.provider_rw().unwrap();
    writer.write_chunked_code(Address::ZERO, account(hash), &code).unwrap();
    writer.commit().unwrap();
    for size in [original.len() as u32, original.len() as u32 + 1] {
        let mut hashes = committed.chunk_hashes().to_vec();
        // Isolate vector disagreement from size disagreement at a mutually valid index.
        if size == original.len() as u32 {
            hashes[0] = B256::repeat_byte(0x42);
        }
        let disagreeing = CodeChunkDescriptor::new(size, hashes).unwrap();
        let state = factory.latest().unwrap();
        assert_chunk_error(
            state
                .get_required_code_chunk(&hash, &CodeRepresentation::Chunked(disagreeing), 0)
                .unwrap_err(),
            ProviderError::CodeChunk(CodeChunkError {
                context: None,
                code_hash: hash,
                index: 0,
                expected_length: Some(24541),
                reason: CodeChunkErrorKind::DescriptorMismatch,
            }),
        );
    }
    let writer = factory.provider_rw().unwrap();
    assert!(writer.tx_ref().delete::<tables::BytecodeChunkDescriptors>(hash, None).unwrap());
    writer.tx_ref().put::<tables::Bytecodes>(hash, Bytecode::new_raw(original)).unwrap();
    writer.commit().unwrap();
    let state = factory.latest().unwrap();
    let representation = CodeRepresentation::Chunked(committed);
    assert_chunk_error(
        state.get_required_code_chunk(&hash, &representation, 0).unwrap_err(),
        ProviderError::CodeChunk(CodeChunkError {
            context: None,
            code_hash: hash,
            index: 0,
            expected_length: Some(24541),
            reason: CodeChunkErrorKind::MissingDescriptor,
        }),
    );
    assert_eq!(state.get_required_code_chunk(&hash, &representation, u32::MAX).unwrap(), None);
}

#[test]
fn t013_reconstruction_authenticates_chunk_and_global_hashes() {
    for mode in 0..4 {
        let factory = create_test_provider_factory();
        let original = fixture();
        let hash = keccak256(&original);
        let code = ValidatedCode::new(original.clone()).unwrap();
        let writer = factory.provider_rw().unwrap();
        writer.write_chunked_code(Address::ZERO, account(hash), &code).unwrap();
        writer.commit().unwrap();
        assert_eq!(
            factory.latest().unwrap().bytecode_by_hash(&hash).unwrap().unwrap().original_bytes(),
            original
        );
        let mut hashes = code.descriptor().unwrap().chunk_hashes().to_vec();
        let writer = factory.provider_rw().unwrap();
        let mut payload = original[..24541].to_vec();
        let expected_reason = match mode {
            0 => {
                payload[1] ^= 0x10;
                writer
                    .tx_ref()
                    .put::<tables::BytecodeChunks>(hashes[0], Bytes::from(payload))
                    .unwrap();
                CodeChunkErrorKind::ChunkHashMismatch
            }
            1 => {
                payload[1] ^= 0x10;
                hashes[0] = keccak256(&payload);
                writer
                    .tx_ref()
                    .put::<tables::BytecodeChunks>(hashes[0], Bytes::from(payload))
                    .unwrap();
                CodeChunkErrorKind::FullCodeHashMismatch
            }
            2 => {
                hashes[0] = B256::repeat_byte(0x42);
                // Install the original bytes under the false key: this reaches commitment checking.
                writer
                    .tx_ref()
                    .put::<tables::BytecodeChunks>(hashes[0], Bytes::from(payload))
                    .unwrap();
                CodeChunkErrorKind::ChunkHashMismatch
            }
            _ => {
                hashes.swap(0, 1);
                CodeChunkErrorKind::FullCodeHashMismatch
            }
        };
        writer
            .tx_ref()
            .put::<tables::BytecodeChunkDescriptors>(
                hash,
                CodeChunkDescriptor::new(original.len() as u32, hashes).unwrap(),
            )
            .unwrap();
        writer.commit().unwrap();
        let error = factory.latest().unwrap().bytecode_by_hash(&hash).unwrap_err();
        let ProviderError::CodeChunk(error) = error else { panic!("unexpected error: {error:?}") };
        assert_eq!(error.code_hash, hash);
        assert_eq!(error.reason, expected_reason);
    }
}

#[test]
fn t029_missing_required_legacy_row_is_not_empty() {
    let factory = create_test_provider_factory();
    let original = Bytes::from_static(&[0x60]);
    let hash = keccak256(&original);
    let writer = factory.provider_rw().unwrap();
    writer.tx_ref().put::<tables::PlainAccountState>(Address::ZERO, account(hash)).unwrap();
    writer.tx_ref().put::<tables::Bytecodes>(hash, Bytecode::new_raw(original)).unwrap();
    writer.commit().unwrap();
    let writer = factory.provider_rw().unwrap();
    assert!(writer.tx_ref().delete::<tables::Bytecodes>(hash, None).unwrap());
    writer.commit().unwrap();
    let state = factory.latest().unwrap();
    assert_eq!(state.get_code_chunk_by_hash(&hash, 0).unwrap(), None);
    assert_chunk_error(
        state.get_required_code_chunk(&hash, &CodeRepresentation::Legacy, 0).unwrap_err(),
        ProviderError::CodeChunk(CodeChunkError {
            context: None,
            code_hash: hash,
            index: 0,
            expected_length: None,
            reason: CodeChunkErrorKind::MissingLegacyCode,
        }),
    );
    assert_eq!(state.get_required_code_chunk(&hash, &CodeRepresentation::Legacy, 1).unwrap(), None);
    assert_eq!(
        state.get_required_code_chunk(&keccak256([]), &CodeRepresentation::Empty, 0).unwrap(),
        None
    );
}

fn account(hash: B256) -> Account {
    Account { bytecode_hash: Some(hash), ..Default::default() }
}

fn fixture() -> Bytes {
    let mut bytes = vec![0; 49089];
    for (index, chunk) in bytes.chunks_mut(24541).enumerate() {
        chunk[0] = 0x60;
        chunk[1] = index as u8;
    }
    bytes[49088] = 0x01;
    bytes.into()
}

fn assert_chunk_error(actual: ProviderError, expected: ProviderError) {
    let ProviderError::CodeChunk(actual) = actual else { panic!("unexpected error: {actual:?}") };
    let ProviderError::CodeChunk(expected) = expected else { unreachable!() };
    assert_eq!(actual.code_hash, expected.code_hash);
    assert_eq!(actual.index, expected.index);
    assert_eq!(actual.expected_length, expected.expected_length);
    assert_eq!(actual.reason, expected.reason);
    assert_eq!(actual.context, expected.context);
}

/// T-013: changing only the declared size must fail the derived final-length invariant.
#[test]
fn t013_reconstruction_rejects_changed_size_with_original_commitments() {
    for size in [49088, 49090] {
        let factory = create_test_provider_factory();
        let original = fixture();
        let hash = keccak256(&original);
        let code = ValidatedCode::new(original.clone()).unwrap();
        let writer = factory.provider_rw().unwrap();
        writer.write_chunked_code(Address::ZERO, account(hash), &code).unwrap();
        writer.commit().unwrap();
        assert_eq!(
            factory.latest().unwrap().bytecode_by_hash(&hash).unwrap().unwrap().original_bytes(),
            original
        );
        let writer = factory.provider_rw().unwrap();
        writer
            .tx_ref()
            .put::<tables::BytecodeChunkDescriptors>(
                hash,
                CodeChunkDescriptor::new(size, code.descriptor().unwrap().chunk_hashes().to_vec())
                    .unwrap(),
            )
            .unwrap();
        writer.commit().unwrap();
        assert_chunk_error(
            factory.latest().unwrap().bytecode_by_hash(&hash).unwrap_err(),
            ProviderError::CodeChunk(CodeChunkError {
                context: None,
                code_hash: hash,
                index: 2,
                expected_length: Some(size as usize - 49082),
                reason: CodeChunkErrorKind::InvalidLength { actual: 7 },
            }),
        );
    }
}

/// T-013: storage validation does not classify original bytes as delegation.
#[test]
fn t013_ef01_multichunk_reconstruction_preserves_legacy_kind() {
    for (size, prefix) in [(24542, 0), (24564, 24541), (49083, 24541)] {
        for imported in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let factory = open_factory(directory.path(), true);
            let mut original = vec![0; size];
            original[prefix..prefix + 2].copy_from_slice(&[0xef, 0x01]);
            if size == 24564 {
                original[prefix + 3..].fill(0x11);
            }
            let original = Bytes::from(original);
            let hash = keccak256(&original);
            let payloads = original.chunks(24541).map(Bytes::copy_from_slice).collect::<Vec<_>>();
            let hashes = payloads.iter().map(keccak256).collect::<Vec<_>>();
            let writer = factory.provider_rw().unwrap();
            if imported {
                writer
                    .import_chunked_code(
                        Address::ZERO,
                        account(hash),
                        size as u32,
                        hashes.clone(),
                        payloads.clone(),
                    )
                    .unwrap();
            } else {
                let validated = ValidatedCode::new(original.clone()).unwrap();
                assert_eq!(validated.descriptor().unwrap().chunk_hashes(), hashes);
                writer.write_chunked_code(Address::ZERO, account(hash), &validated).unwrap();
            }
            writer.commit().unwrap();
            drop(factory);
            let factory = open_factory(directory.path(), false);
            let reader = factory.provider().unwrap();
            assert!(reader.tx_ref().get::<tables::Bytecodes>(hash).unwrap().is_none());
            drop(reader);
            let state = factory.latest().unwrap();
            for (index, expected) in payloads.iter().enumerate() {
                assert_eq!(
                    state.get_code_chunk_by_hash(&hash, index as u32).unwrap().as_ref(),
                    Some(expected)
                );
            }
            let reconstructed = state.bytecode_by_hash(&hash).unwrap().unwrap();
            assert!(reconstructed.0.is_legacy());
            assert!(!reconstructed.0.is_eip7702());
            assert_eq!(reconstructed.original_bytes(), original);
            assert_eq!(keccak256(reconstructed.original_bytes()), hash);
        }
    }
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

/// T-029: account-based reads require content even when discovery cannot find a descriptor.
#[test]
fn t029_account_code_requires_surviving_legacy_accounts_content() {
    assert_required_account_content(false);
}

/// T-029: loss of metadata does not erase the surviving account's code obligation.
#[test]
fn t029_account_code_requires_surviving_chunked_accounts_content() {
    assert_required_account_content(true);
}

fn assert_required_account_content(multi: bool) {
    let factory = create_test_provider_factory();
    let original = if multi { fixture() } else { Bytes::from_static(&[0x60]) };
    let hash = keccak256(&original);
    let code = ValidatedCode::new(original.clone()).unwrap();
    let owner = Address::with_last_byte(29);
    let writer = factory.provider_rw().unwrap();
    writer.write_chunked_code(owner, account(hash), &code).unwrap();
    writer
        .tx_ref()
        .put::<tables::PlainAccountState>(Address::with_last_byte(30), Account::default())
        .unwrap();
    writer
        .tx_ref()
        .put::<tables::PlainAccountState>(Address::with_last_byte(31), account(keccak256([])))
        .unwrap();
    writer.commit().unwrap();
    assert_eq!(
        factory.latest().unwrap().account_code(&owner).unwrap().unwrap().original_bytes(),
        original
    );
    let writer = factory.provider_rw().unwrap();
    if multi {
        assert!(writer.tx_ref().delete::<tables::BytecodeChunkDescriptors>(hash, None).unwrap());
    } else {
        assert!(writer.tx_ref().delete::<tables::Bytecodes>(hash, None).unwrap());
    }
    writer.commit().unwrap();
    let reader = factory.provider().unwrap();
    assert_eq!(
        reader.tx_ref().get::<tables::PlainAccountState>(owner).unwrap(),
        Some(account(hash))
    );
    assert_eq!(reader.tx_ref().get::<tables::Bytecodes>(hash).unwrap(), None);
    assert_eq!(reader.tx_ref().get::<tables::BytecodeChunkDescriptors>(hash).unwrap(), None);
    drop(reader);
    let state = factory.latest().unwrap();
    assert_eq!(state.bytecode_by_hash(&hash).unwrap(), None);
    assert_eq!(state.bytecode_by_hash(&B256::repeat_byte(0x91)).unwrap(), None);
    for empty in [Address::ZERO, Address::with_last_byte(30), Address::with_last_byte(31)] {
        assert_eq!(state.account_code(&empty).unwrap(), None);
    }
    let error = state.account_code(&owner).unwrap_err();
    let ProviderError::CodeChunk(error) = error else { panic!("unexpected error: {error:?}") };
    assert_eq!(error.code_hash, hash);
    assert_eq!(error.index, 0);
    assert_eq!(error.expected_length, None);
    assert_eq!(error.context, None);
    // With no descriptor, the provider knows only that required code is missing.
    // This exact reason deliberately does not assert a legacy or chunked representation.
    assert_eq!(format!("{:?}", error.reason), "MissingCode");
    drop(state);
    let writer = factory.provider_rw().unwrap();
    writer.write_chunked_code(owner, account(hash), &code).unwrap();
    writer.commit().unwrap();
    assert_eq!(
        factory.latest().unwrap().account_code(&owner).unwrap().unwrap().original_bytes(),
        original
    );
}

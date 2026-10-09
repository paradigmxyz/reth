//! T-008, T-009, T-016: immutable base-format fixtures and legacy reads.
//!
//! Fixture bytes below were emitted by the unmodified production codecs at
//! f9f1f9766064a407617c84a0feed4db98e8cbc7b using nonce=1, balance=2,
//! address=0, and original bytecode [0x60]. Captured before implementation with
//! cargo test -p reth-provider --test tip1143_compatibility and account-ext/test-utils.
//! Account Compact, bytecode Compact, trie RLP, and root are independent frozen oracles.

use alloy_consensus::Header;
use alloy_primitives::{b256, hex, keccak256, Address, Bytes, B256, U256};
use reth_chainspec::MAINNET;
use reth_config::config::EtlConfig;
use reth_db::{
    init_db,
    mdbx::{ffi, DatabaseArguments},
    open_db,
    test_utils::DatabaseTestHooks,
    DatabaseEnv,
};
use reth_db_api::{
    table::{Compress, Decompress},
    tables::{self, RawTable, RawValue},
    transaction::{DbTx, DbTxMut},
};
use reth_db_common::{init::init_from_state_dump, DbTool};
use reth_primitives_traits::{Account, Bytecode, SealedHeader};
use reth_provider::{
    providers::RocksDBBuilder,
    test_utils::{
        create_test_provider_factory, create_test_provider_factory_with_db_hooks, insert_headers,
        MockNodeTypesWithDB,
    },
    ProviderFactory, StateProviderFactory, StaticFileProviderBuilder,
};
use reth_storage_api::{
    AccountReader, BytecodeReader, CodeChunkReader, CodeRepresentation, DBProvider, ValidatedCode,
};
use reth_trie::{root::state_root_unhashed, EMPTY_ROOT_HASH};
use std::{ffi::CString, path::Path, sync::Arc};

const ACCOUNT: &[u8] =
    &hex!("1104010215a5de5d00dfc39d199ee772e89858c204d1d545de092db54a345c7303942607");
const BYTECODE: &[u8] = &hex!("0000000360000002000000000000000100");
const RLP: &[u8] = &hex!("f8440102a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a015a5de5d00dfc39d199ee772e89858c204d1d545de092db54a345c7303942607");

#[test]
fn t008_original_bytecodes_serve_legacy_zero() {
    for size in [1, 24575, 24576] {
        let hooks = DatabaseTestHooks::default();
        let factory = create_test_provider_factory_with_db_hooks(hooks.clone());
        let mut original = vec![0; size];
        original[size - 1] = 0x7f;
        let hash = keccak256(&original);
        let bytecode = Bytecode::new_raw(original.clone().into());
        let before = bytecode.clone().compress();
        let writer = factory.provider_rw().unwrap();
        writer.tx_ref().put::<tables::Bytecodes>(hash, bytecode).unwrap();
        writer.commit().unwrap();
        hooks.clear_reads();
        let control = factory.provider().unwrap();
        control.tx_ref().get::<tables::Bytecodes>(hash).unwrap().unwrap();
        assert_eq!(hooks.reads().iter().filter(|r| r.table == "Bytecodes").count(), 1);
        drop(control);
        hooks.clear_reads();
        let state = factory.latest().unwrap();
        assert_eq!(state.get_code_chunk_by_hash(&hash, 0).unwrap(), Some(Bytes::from(original)));
        assert_eq!(hooks.reads().iter().filter(|r| r.table == "Bytecodes").count(), 1);
        assert!(!hooks.reads().iter().any(|r| r.table == "BytecodeChunks"));
        for index in [1, u32::MAX] {
            hooks.clear_reads();
            assert_eq!(state.get_code_chunk_by_hash(&hash, index).unwrap(), None);
            assert!(!hooks
                .reads()
                .iter()
                .any(|r| r.table == "Bytecodes" || r.table == "BytecodeChunks"));
            hooks.clear_reads();
            assert_eq!(
                state.get_required_code_chunk(&hash, &CodeRepresentation::Legacy, index).unwrap(),
                None
            );
            assert!(hooks.reads().is_empty());
        }
        for index in [0, 1, u32::MAX] {
            hooks.clear_reads();
            assert_eq!(state.get_code_chunk_by_hash(&keccak256([]), index).unwrap(), None);
            assert!(hooks.reads().is_empty());
        }
        let reader = factory.provider().unwrap();
        assert_eq!(
            reader
                .tx_ref()
                .get::<RawTable<tables::Bytecodes>>(hash.into())
                .unwrap()
                .unwrap()
                .raw_value(),
            before.as_slice()
        );
        assert_eq!(reader.tx_ref().entries::<tables::BytecodeChunkDescriptors>().unwrap(), 0);
        assert_eq!(reader.tx_ref().entries::<tables::BytecodeChunks>().unwrap(), 0);
    }
}

#[test]
fn t009_oversized_legacy_record_remains_unsplit() {
    let factory = create_test_provider_factory();
    let original = Bytes::from(vec![1; 24577]);
    let hash = keccak256(&original);
    let bytecode = Bytecode::new_raw(original.clone());
    let before = bytecode.clone().compress();
    let writer = factory.provider_rw().unwrap();
    writer.tx_ref().put::<tables::Bytecodes>(hash, bytecode).unwrap();
    writer.commit().unwrap();
    let state = factory.latest().unwrap();
    assert_eq!(state.bytecode_by_hash(&hash).unwrap().unwrap().original_bytes(), original);
    assert!(state.get_code_chunk_by_hash(&hash, 0).is_err());
    assert_eq!(state.get_required_code_chunk(&hash, &CodeRepresentation::Legacy, 1).unwrap(), None);
    let reader = factory.provider().unwrap();
    assert_eq!(
        reader
            .tx_ref()
            .get::<RawTable<tables::Bytecodes>>(hash.into())
            .unwrap()
            .unwrap()
            .raw_value(),
        before.as_slice()
    );
    assert_eq!(reader.tx_ref().entries::<tables::BytecodeChunkDescriptors>().unwrap(), 0);
    assert_eq!(reader.tx_ref().entries::<tables::BytecodeChunks>().unwrap(), 0);
}

#[test]
fn t016_base_compact_rlp_and_state_commitment_are_unchanged() {
    let factory = create_test_provider_factory();
    let hash = keccak256([0x60]);
    let account = Account {
        nonce: 1,
        balance: U256::from(2),
        bytecode_hash: Some(hash),
        ..Default::default()
    };
    assert_eq!(Account::decompress(ACCOUNT).unwrap(), account);
    let trie = account.clone().into_trie_account(EMPTY_ROOT_HASH);
    assert_eq!(alloy_rlp::encode(trie.clone()), RLP);
    assert_eq!(
        state_root_unhashed([(Address::ZERO, trie)]),
        b256!("bdde96c55730dea2f8125793c6b032892aa1b923da2fb898ea39884f66bf2ede")
    );
    let writer = factory.provider_rw().unwrap();
    writer
        .tx_ref()
        .put::<RawTable<tables::PlainAccountState>>(
            Address::ZERO.into(),
            RawValue::from_vec(ACCOUNT.to_vec()),
        )
        .unwrap();
    writer
        .tx_ref()
        .put::<RawTable<tables::Bytecodes>>(hash.into(), RawValue::from_vec(BYTECODE.to_vec()))
        .unwrap();
    writer.commit().unwrap();
    assert_eq!(
        factory.latest().unwrap().get_code_chunk_by_hash(&hash, 0).unwrap(),
        Some(Bytes::from_static(&[0x60]))
    );
    let writer = factory.provider_rw().unwrap();
    for (address, bytes) in [
        (Address::repeat_byte(1), Bytes::from(vec![0; 24577])),
        (Address::repeat_byte(2), Bytes::from_static(&[1])),
        (Address::repeat_byte(3), Bytes::new()),
    ] {
        let code = ValidatedCode::new(bytes.clone()).unwrap();
        writer
            .write_chunked_code(
                address,
                Account {
                    bytecode_hash: (!bytes.is_empty()).then(|| keccak256(&bytes)),
                    ..Default::default()
                },
                &code,
            )
            .unwrap();
    }
    writer.commit().unwrap();
    let reader = factory.provider().unwrap();
    assert_eq!(
        reader
            .tx_ref()
            .get::<RawTable<tables::PlainAccountState>>(Address::ZERO.into())
            .unwrap()
            .unwrap()
            .raw_value(),
        ACCOUNT
    );
    assert_eq!(
        reader
            .tx_ref()
            .get::<RawTable<tables::Bytecodes>>(hash.into())
            .unwrap()
            .unwrap()
            .raw_value(),
        BYTECODE
    );
    assert!(reader
        .tx_ref()
        .get::<tables::BytecodeChunkDescriptors>(keccak256([1]))
        .unwrap()
        .is_none());
    assert!(reader
        .tx_ref()
        .get::<tables::BytecodeChunkDescriptors>(keccak256([]))
        .unwrap()
        .is_none());
    let untouched =
        reader.tx_ref().get::<tables::PlainAccountState>(Address::ZERO).unwrap().unwrap();
    assert_eq!(alloy_rlp::encode(untouched.into_trie_account(EMPTY_ROOT_HASH)), RLP);

    // Build expected leaves directly from literal fields, bypassing Account conversion,
    // then independently assemble the hexary trie. The old leaf remains pinned above.
    let empty_storage = hex!("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421");
    let mut expected = vec![(Address::ZERO, RLP.to_vec())];
    for (address, bytes) in [
        (Address::repeat_byte(1), vec![0; 24577]),
        (Address::repeat_byte(2), vec![1]),
        (Address::repeat_byte(3), vec![]),
    ] {
        expected.push((
            address,
            oracle_rlp_list(&[
                oracle_rlp_bytes(&[]),
                oracle_rlp_bytes(&[]),
                oracle_rlp_bytes(&empty_storage),
                oracle_rlp_bytes(keccak256(bytes).as_slice()),
            ]),
        ));
    }
    let expected_root = oracle_state_root(&expected);
    let persisted = expected
        .iter()
        .map(|(address, _)| {
            let account =
                reader.tx_ref().get::<tables::PlainAccountState>(*address).unwrap().unwrap();
            (*address, account.into_trie_account(EMPTY_ROOT_HASH))
        })
        .collect::<Vec<_>>();
    assert_eq!(state_root_unhashed(persisted.clone()), expected_root);
    assert_ne!(expected_root, oracle_state_root(&expected[..1]));
    // Positive control: changing a committed field must change the expected root.
    expected[1].1 = RLP.to_vec();
    assert_ne!(state_root_unhashed(persisted), oracle_state_root(&expected));
}

/// T-024: restore the literal base-format snapshot through the production importer.
/// The root and original account bytes use the pinned T-016 base oracle above.
/// The JSON is the existing public state-dump wire format, not a test table copier.
#[test]
fn t024_restore_legacy_state_dump_then_ingest_chunked_state() {
    let directory = tempfile::tempdir().unwrap();
    let factory = lifecycle_factory(directory.path(), true);
    let root = b256!("bdde96c55730dea2f8125793c6b032892aa1b923da2fb898ea39884f66bf2ede");
    let block_hash = keccak256(b"T-024 snapshot block");
    insert_headers(
        &factory,
        &[SealedHeader::new(Header { state_root: root, ..Default::default() }, block_hash)],
    );
    let dump = concat!(
        "{\"root\":\"0xbdde96c55730dea2f8125793c6b032892aa1b923da2fb898ea39884f66bf2ede\"}\n",
        "{\"address\":\"0x0000000000000000000000000000000000000000\",",
        "\"nonce\":\"0x1\",\"balance\":\"0x2\",\"code\":\"0x60\",\"storage\":{}}\n"
    );
    let etl = tempfile::tempdir().unwrap();
    assert_eq!(
        init_from_state_dump(
            dump.as_bytes(),
            &factory,
            EtlConfig { dir: Some(etl.path().to_path_buf()), file_size: 1024 },
        )
        .unwrap(),
        block_hash
    );
    drop(factory);
    let factory = lifecycle_factory(directory.path(), false);
    let hash = keccak256([0x60]);
    let reader = factory.provider().unwrap();
    assert_eq!(
        reader
            .tx_ref()
            .get::<RawTable<tables::PlainAccountState>>(Address::ZERO.into())
            .unwrap()
            .unwrap()
            .raw_value(),
        ACCOUNT
    );
    let legacy_row = reader
        .tx_ref()
        .get::<RawTable<tables::Bytecodes>>(hash.into())
        .unwrap()
        .unwrap()
        .raw_value()
        .to_vec();
    assert_eq!(reader.tx_ref().entries::<tables::BytecodeChunkDescriptors>().unwrap(), 0);
    drop(reader);
    let old = factory.latest().unwrap();
    let old_account = old.basic_account(&Address::ZERO).unwrap().unwrap();
    assert_eq!(alloy_rlp::encode(old_account.clone().into_trie_account(EMPTY_ROOT_HASH)), RLP);
    assert_eq!(
        state_root_unhashed([(Address::ZERO, old_account.into_trie_account(EMPTY_ROOT_HASH))]),
        root
    );
    assert_eq!(old.get_code_chunk_by_hash(&hash, 0).unwrap(), Some(Bytes::from_static(&[0x60])));
    let bytes = Bytes::from(vec![0; 24542]);
    let new_hash = keccak256(&bytes);
    let chunks = bytes.chunks(24541).map(Bytes::copy_from_slice).collect::<Vec<_>>();
    let writer = factory.provider_rw().unwrap();
    writer
        .import_chunked_code(
            Address::repeat_byte(0x24),
            Account { bytecode_hash: Some(new_hash), ..Default::default() },
            24542,
            chunks.iter().map(keccak256).collect(),
            chunks,
        )
        .unwrap();
    writer.commit().unwrap();
    assert_eq!(old.basic_account(&Address::repeat_byte(0x24)).unwrap(), None);
    drop(old);
    drop(factory);
    let factory = lifecycle_factory(directory.path(), false);
    let state = factory.latest().unwrap();
    assert_eq!(state.bytecode_by_hash(&new_hash).unwrap().unwrap().original_bytes(), bytes);
    assert_eq!(state.get_code_chunk_by_hash(&new_hash, 1).unwrap(), Some(Bytes::from_static(&[0])));
    assert_eq!(
        state.bytecode_by_hash(&hash).unwrap().unwrap().original_bytes(),
        Bytes::from_static(&[0x60])
    );
    let reader = factory.provider().unwrap();
    assert_eq!(
        reader
            .tx_ref()
            .get::<RawTable<tables::PlainAccountState>>(Address::ZERO.into())
            .unwrap()
            .unwrap()
            .raw_value(),
        ACCOUNT
    );
    assert_eq!(
        reader
            .tx_ref()
            .get::<RawTable<tables::Bytecodes>>(hash.into())
            .unwrap()
            .unwrap()
            .raw_value(),
        legacy_row
    );
}

/// T-024: partial maintenance of live chunk storage must reject before mutation.
/// This binds the real CLI DbTool entry point; no substitute maintenance adapter.
#[test]
fn t024_table_maintenance_cannot_strand_live_chunked_accounts() {
    let factory = create_test_provider_factory();
    let bytes = Bytes::from(vec![0; 24542]);
    let hash = keccak256(&bytes);
    let address = Address::repeat_byte(0x24);
    let account = Account { nonce: 9, bytecode_hash: Some(hash), ..Default::default() };
    let writer = factory.provider_rw().unwrap();
    writer
        .write_chunked_code(address, account.clone(), &ValidatedCode::new(bytes.clone()).unwrap())
        .unwrap();
    writer.commit().unwrap();
    let tool = DbTool::new(factory.clone()).unwrap();
    for descriptor in [false, true] {
        let error = if descriptor {
            tool.drop_table::<tables::BytecodeChunkDescriptors>().unwrap_err()
        } else {
            tool.drop_table::<tables::BytecodeChunks>().unwrap_err()
        };
        assert_eq!(
            error.to_string(),
            "cannot drop chunk storage while chunked accounts are retained"
        );
        let state = factory.latest().unwrap();
        assert_eq!(state.basic_account(&address).unwrap(), Some(account.clone()));
        assert_eq!(state.bytecode_by_hash(&hash).unwrap().unwrap().original_bytes(), bytes);
        for (i, expected) in bytes.chunks(24541).enumerate() {
            assert_eq!(
                state.get_code_chunk_by_hash(&hash, i as u32).unwrap().unwrap().as_ref(),
                expected
            );
        }
    }
    // A genuinely empty database remains maintainable through the same entry point.
    let empty = create_test_provider_factory();
    let tool = DbTool::new(empty).unwrap();
    tool.drop_table::<tables::BytecodeChunks>().unwrap();
    tool.drop_table::<tables::BytecodeChunkDescriptors>().unwrap();
}

fn lifecycle_factory(
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

/// T-024: the production whole-database drop removes descriptors and payloads together.
#[test]
fn t024_whole_database_drop_does_not_leave_reusable_chunk_state() {
    let directory = tempfile::tempdir().unwrap();
    let factory = lifecycle_factory(directory.path(), true);
    let original = Bytes::from(vec![0; 24542]);
    let hash = keccak256(&original);
    let address = Address::repeat_byte(0x24);
    let writer = factory.provider_rw().unwrap();
    writer
        .write_chunked_code(
            address,
            Account { bytecode_hash: Some(hash), ..Default::default() },
            &ValidatedCode::new(original.clone()).unwrap(),
        )
        .unwrap();
    writer.commit().unwrap();
    assert_eq!(
        factory.latest().unwrap().bytecode_by_hash(&hash).unwrap().unwrap().original_bytes(),
        original
    );
    let tool = DbTool::new(factory.clone()).unwrap();
    tool.drop(
        directory.path().join("db"),
        directory.path().join("static_files"),
        directory.path().join("exex_wal"),
    )
    .unwrap();
    drop(tool);
    drop(factory);
    assert!(!directory.path().join("db").exists());
    let fresh = lifecycle_factory(directory.path(), true);
    let state = fresh.latest().unwrap();
    assert_eq!(state.basic_account(&address).unwrap(), None);
    assert_eq!(state.bytecode_by_hash(&hash).unwrap(), None);
    assert_eq!(state.get_code_chunk_by_hash(&hash, 0).unwrap(), None);
    drop(state);
    let reader = fresh.provider().unwrap();
    assert_eq!(reader.tx_ref().get::<tables::BytecodeChunkDescriptors>(hash).unwrap(), None);
    for payload in original.chunks(24541) {
        assert_eq!(
            reader.tx_ref().get::<tables::BytecodeChunks>(keccak256(payload)).unwrap(),
            None
        );
    }
}

/// T-024: exercise the production MDBX copy primitive used by `reth db copy`.
/// Both ordinary and compact copies must retain every table without a table allowlist.
#[test]
fn t024_database_copy_reopens_legacy_and_chunked_records() {
    for compact in [false, true] {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let factory = lifecycle_factory(source.path(), true);
        let legacy_hash = keccak256([0x60]);
        let mut original = vec![0; 49083];
        original[0] = 0x60;
        original[1] = 0x24;
        original[24541] = 0x60;
        original[24542] = 0x42;
        original[49082] = 0x01;
        let hash = keccak256(&original);
        let address = Address::repeat_byte(0x24);
        let account = Account { nonce: 7, bytecode_hash: Some(hash), ..Default::default() };
        let writer = factory.provider_rw().unwrap();
        writer
            .tx_ref()
            .put::<RawTable<tables::PlainAccountState>>(
                Address::ZERO.into(),
                RawValue::from_vec(ACCOUNT.to_vec()),
            )
            .unwrap();
        writer
            .tx_ref()
            .put::<RawTable<tables::Bytecodes>>(
                legacy_hash.into(),
                RawValue::from_vec(BYTECODE.to_vec()),
            )
            .unwrap();
        writer
            .write_chunked_code(
                address,
                account.clone(),
                &ValidatedCode::new(original.clone().into()).unwrap(),
            )
            .unwrap();
        writer.commit().unwrap();
        let copied_db = destination.path().join("db");
        reth_fs_util::create_dir_all(&copied_db).unwrap();
        let path = CString::new(copied_db.join("mdbx.dat").to_str().unwrap()).unwrap();
        let flags = if compact { ffi::MDBX_CP_COMPACT } else { ffi::MDBX_CP_DEFAULTS };
        // SAFETY: the environment remains open for the callback and path is a live,
        // NUL-terminated string. MDBX owns the copy operation and no pointer escapes.
        let result = factory
            .db_ref()
            .with_raw_env_ptr(|env| unsafe { ffi::mdbx_env_copy(env, path.as_ptr(), flags) });
        assert_eq!(result, 0);
        drop(factory);
        // Only the copied MDBX environment is reused: no source handles or caches survive.
        let restored = lifecycle_factory(destination.path(), false);
        let state = restored.latest().unwrap();
        assert_eq!(state.basic_account(&address).unwrap(), Some(account));
        assert_eq!(
            state.bytecode_by_hash(&hash).unwrap().unwrap().original_bytes().as_ref(),
            original
        );
        assert_eq!(
            state.get_code_chunk_by_hash(&legacy_hash, 0).unwrap(),
            Some(Bytes::from_static(&[0x60]))
        );
        for (index, payload) in original.chunks(24541).enumerate() {
            assert_eq!(
                state.get_code_chunk_by_hash(&hash, index as u32).unwrap().unwrap().as_ref(),
                payload
            );
        }
        let reader = restored.provider().unwrap();
        assert!(reader.tx_ref().get::<tables::Bytecodes>(hash).unwrap().is_none());
        assert_eq!(
            reader
                .tx_ref()
                .get::<RawTable<tables::PlainAccountState>>(Address::ZERO.into())
                .unwrap()
                .unwrap()
                .raw_value(),
            ACCOUNT
        );
        assert_eq!(
            reader
                .tx_ref()
                .get::<RawTable<tables::Bytecodes>>(legacy_hash.into())
                .unwrap()
                .unwrap()
                .raw_value(),
            BYTECODE
        );
    }
}

/// Independent small-fixture MPT oracle, with no production trie builder or account codec.
fn oracle_state_root(accounts: &[(Address, Vec<u8>)]) -> B256 {
    let leaves = accounts
        .iter()
        .map(|(address, value)| {
            let key = keccak256(address);
            let nibbles = key.iter().flat_map(|byte| [byte >> 4, byte & 15]).collect::<Vec<_>>();
            (nibbles, value.clone())
        })
        .collect::<Vec<_>>();
    keccak256(oracle_trie_node(&leaves, 0))
}

fn oracle_trie_node(leaves: &[(Vec<u8>, Vec<u8>)], depth: usize) -> Vec<u8> {
    assert!(!leaves.is_empty());
    if leaves.len() == 1 {
        return oracle_rlp_list(&[
            oracle_rlp_bytes(&oracle_compact_path(&leaves[0].0[depth..], true)),
            oracle_rlp_bytes(&leaves[0].1),
        ]);
    }
    let common = (depth..64)
        .take_while(|index| leaves.iter().all(|leaf| leaf.0[*index] == leaves[0].0[*index]))
        .count();
    if common > 0 {
        let child = oracle_trie_node(leaves, depth + common);
        return oracle_rlp_list(&[
            oracle_rlp_bytes(&oracle_compact_path(&leaves[0].0[depth..depth + common], false)),
            oracle_child_reference(child),
        ]);
    }
    let mut children = Vec::new();
    for nibble in 0..16 {
        let group =
            leaves.iter().filter(|leaf| leaf.0[depth] == nibble).cloned().collect::<Vec<_>>();
        children.push(if group.is_empty() {
            oracle_rlp_bytes(&[])
        } else {
            oracle_child_reference(oracle_trie_node(&group, depth + 1))
        });
    }
    children.push(oracle_rlp_bytes(&[]));
    oracle_rlp_list(&children)
}

fn oracle_compact_path(nibbles: &[u8], leaf: bool) -> Vec<u8> {
    let odd = nibbles.len() % 2;
    let mut bytes = vec![(u8::from(leaf) * 2 + odd as u8) << 4];
    if odd == 1 {
        bytes[0] |= nibbles[0];
    }
    bytes.extend(nibbles[odd..].chunks_exact(2).map(|pair| pair[0] * 16 + pair[1]));
    bytes
}

fn oracle_child_reference(encoded: Vec<u8>) -> Vec<u8> {
    if encoded.len() < 32 {
        encoded
    } else {
        oracle_rlp_bytes(keccak256(encoded).as_slice())
    }
}

fn oracle_rlp_bytes(bytes: &[u8]) -> Vec<u8> {
    if bytes.len() == 1 && bytes[0] < 128 {
        return bytes.to_vec();
    }
    oracle_rlp_frame(bytes, 128)
}

fn oracle_rlp_list(items: &[Vec<u8>]) -> Vec<u8> {
    oracle_rlp_frame(&items.concat(), 192)
}

fn oracle_rlp_frame(payload: &[u8], base: u8) -> Vec<u8> {
    let mut encoded = Vec::new();
    if payload.len() < 56 {
        encoded.push(base + payload.len() as u8);
    } else {
        let length = payload.len().to_be_bytes();
        let first = length.iter().position(|byte| *byte != 0).unwrap();
        encoded.push(base + 55 + (length.len() - first) as u8);
        encoded.extend_from_slice(&length[first..]);
    }
    encoded.extend_from_slice(payload);
    encoded
}

#[test]
fn t016_independent_trie_oracle_matches_pinned_base_root() {
    assert_eq!(
        oracle_state_root(&[(Address::ZERO, RLP.to_vec())]),
        b256!("bdde96c55730dea2f8125793c6b032892aa1b923da2fb898ea39884f66bf2ede")
    );
}

/// T-008: identical prefix bytes do not override the persisted bytecode kind.
#[test]
fn t008_legacy_bytecode_kinds_survive_reads() {
    let target = Address::repeat_byte(0x11);
    let mut marker = vec![0xef, 0x01, 0x00];
    marker.extend_from_slice(target.as_slice());
    for delegation in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let factory = lifecycle_factory(directory.path(), true);
        let original = Bytes::from(marker.clone());
        let hash = keccak256(&original);
        let code = Bytecode(if delegation {
            revm::bytecode::Bytecode::new_eip7702(target)
        } else {
            revm::bytecode::Bytecode::new_legacy(original.clone())
        });
        let encoded = code.clone().compress();
        let writer = factory.provider_rw().unwrap();
        writer.tx_ref().put::<tables::Bytecodes>(hash, code.clone()).unwrap();
        writer.commit().unwrap();
        drop(factory);
        let factory = lifecycle_factory(directory.path(), false);
        let state = factory.latest().unwrap();
        assert_eq!(state.get_code_chunk_by_hash(&hash, 0).unwrap(), Some(original.clone()));
        assert_eq!(
            state.get_required_code_chunk(&hash, &CodeRepresentation::Legacy, 0).unwrap(),
            Some(original)
        );
        let full = state.bytecode_by_hash(&hash).unwrap().unwrap();
        assert_eq!(full, code);
        assert_eq!(full.0.is_eip7702(), delegation);
        assert_eq!(full.0.is_legacy(), !delegation);
        let reader = factory.provider().unwrap();
        assert_eq!(
            reader
                .tx_ref()
                .get::<RawTable<tables::Bytecodes>>(hash.into())
                .unwrap()
                .unwrap()
                .raw_value(),
            encoded.as_slice()
        );
        assert_eq!(reader.tx_ref().entries::<tables::BytecodeChunkDescriptors>().unwrap(), 0);
    }
}

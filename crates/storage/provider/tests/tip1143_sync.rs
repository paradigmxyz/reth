//! T-022: production snap attempts, staging, account publication and completeness.
//!
//! Proposed APIs: SnapBytecodeStore::stage_chunked_code authenticates a complete
//! untrusted submission without publishing accounts. VerifiedAccountRange::verify_response
//! exposes the downloader's existing response verifier, not an unchecked fixture constructor.
//! Chain-composed account extensions are opaque throughout this flow.

use alloy_consensus::Header;
use alloy_eips::BlockNumHash;
use alloy_primitives::{keccak256, Bytes, B256};
use reth_db_api::{
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_downloaders::snap::VerifiedAccountRange;
use reth_eth_wire_types::snap::{AccountData, AccountRangeMessage, GetAccountRangeMessage};
use reth_primitives_traits::{Account, SealedHeader};
use reth_provider::{
    test_utils::{create_test_provider_factory, insert_headers},
    ProviderError, StateProviderFactory,
};
use reth_snap_sync::{
    SnapAccountStore, SnapAttemptStore, SnapBytecodeStore, SnapGeneration, SnapStateVerifier,
    SnapSyncError,
};
use reth_storage_api::{
    BytecodeReader, CodeValidationError, DBProvider, MetadataWriter, StorageSettings,
    StorageSettingsCache,
};
use reth_trie::{root::state_root_unsorted, EMPTY_ROOT_HASH};
use tokio_util::sync::CancellationToken;

#[test]
fn t022_staging_publication_and_handoff_recheck_required_content() {
    let factory = create_test_provider_factory();
    let writer = factory.provider_rw().unwrap();
    writer.write_storage_settings(StorageSettings::v2()).unwrap();
    writer.commit().unwrap();
    factory.set_storage_settings_cache(StorageSettings::v2());
    let mut bytes = vec![0; 49083];
    bytes[0] = 0x60;
    bytes[1] = 0x11;
    bytes[24541] = 0x60;
    bytes[24542] = 0x22;
    bytes[49082] = 0x5b;
    let bytes = Bytes::from(bytes);
    let hash = keccak256(&bytes);
    let chunks = bytes.chunks(24541).map(Bytes::copy_from_slice).collect::<Vec<_>>();
    let hashes = chunks.iter().map(keccak256).collect::<Vec<_>>();
    let key = B256::repeat_byte(0x22);
    let mut account = Account {
        nonce: 7,
        bytecode_hash: Some(hash),
        extension: Bytes::from_static(&[0xfe, 0x11, 0x43]).into(),
        ..Default::default()
    };
    let metadata = reth_execution_types::encode_code_metadata(&evm2::evm::AccountInfo {
        code_hash: hash,
        code_metadata: evm2::bytecode::code_metadata(&bytes).unwrap(),
        ..Default::default()
    })
    .unwrap();
    account.extension = account.extension.with_code_metadata(&metadata);
    let trie_account = account.clone().into_trie_account(EMPTY_ROOT_HASH);
    let root = state_root_unsorted([(key, trie_account.clone())]);
    let pivot = B256::repeat_byte(1);
    insert_headers(
        &factory,
        &[
            SealedHeader::new(Header::default(), B256::ZERO),
            SealedHeader::new(Header { number: 1, state_root: root, ..Default::default() }, pivot),
        ],
    );
    let writer = factory.provider_rw().unwrap();
    let write =
        writer.start_snap_attempt(SnapGeneration::new(BlockNumHash::new(1, pivot), root)).unwrap();
    writer.start_account_coverage(write).unwrap();
    writer.commit().unwrap();
    let range = VerifiedAccountRange::verify_response(
        GetAccountRangeMessage {
            request_id: 1,
            root_hash: root,
            starting_hash: B256::ZERO,
            limit_hash: B256::repeat_byte(0xff),
            response_bytes: 1_000_000,
        },
        AccountRangeMessage {
            request_id: 1,
            accounts: vec![AccountData::from_trie_account(key, &trie_account)],
            proof: vec![],
        },
    )
    .unwrap();
    assert_eq!(range.accounts(), &[(key, trie_account)]);
    assert_eq!(range.next(), None);
    let writer = factory.provider_rw().unwrap();
    assert!(matches!(writer.commit_account_range(write, &range, Default::default(), vec![]),
        Err(SnapSyncError::MissingCode { hash: missing }) if missing == hash));
    assert_eq!(writer.tx_ref().get::<tables::HashedAccounts>(key).unwrap(), None);
    // Wrong payload bytes with the original commitments must leave no staging rows.
    let mut wrong = chunks.clone();
    wrong[2] = Bytes::from_static(&[0x01]);
    assert!(matches!(
        writer.stage_chunked_code(write, hash, 49083, hashes.clone(), wrong),
        Err(SnapSyncError::Provider(ProviderError::InvalidChunkedCode(
            CodeValidationError::ChunkHash { index: 2 }
        )))
    ));
    assert_eq!(writer.tx_ref().entries::<tables::BytecodeChunks>().unwrap(), 0);
    assert_eq!(writer.tx_ref().entries::<tables::BytecodeChunkDescriptors>().unwrap(), 0);
    writer.stage_chunked_code(write, hash, 49083, hashes.clone(), chunks.clone()).unwrap();
    assert_eq!(writer.tx_ref().get::<tables::HashedAccounts>(key).unwrap(), None);
    assert_eq!(writer.missing_code(write, &[hash], 1).unwrap(), Vec::<B256>::new());
    writer.commit().unwrap();
    let writer = factory.provider_rw().unwrap();
    writer.tx_ref().delete::<tables::BytecodeChunks>(hashes[1], None).unwrap();
    assert_eq!(writer.missing_code(write, &[hash], 1).unwrap(), vec![hash]);
    assert!(matches!(writer.commit_account_range(write, &range, Default::default(), vec![]),
        Err(SnapSyncError::MissingCode { hash: missing }) if missing == hash));
    assert_eq!(writer.tx_ref().get::<tables::HashedAccounts>(key).unwrap(), None);
    writer.stage_chunked_code(write, hash, 49083, hashes.clone(), chunks.clone()).unwrap();
    writer.commit_account_range(write, &range, Default::default(), vec![]).unwrap();
    // A new reader while publication is pending must not see the account.
    assert_eq!(
        factory.provider().unwrap().tx_ref().get::<tables::HashedAccounts>(key).unwrap(),
        None
    );
    let cancel = CancellationToken::new();
    writer.verify_completeness(write, 1, &cancel).unwrap();
    writer.commit().unwrap();
    let reader = factory.provider().unwrap();
    assert_eq!(reader.tx_ref().get::<tables::HashedAccounts>(key).unwrap(), Some(account));
    assert_eq!(reader.tx_ref().get::<tables::Bytecodes>(hash).unwrap(), None);
    drop(reader);
    assert_eq!(
        factory.latest().unwrap().bytecode_by_hash(&hash).unwrap().unwrap().original_bytes(),
        bytes
    );
    let writer = factory.provider_rw().unwrap();
    writer.tx_ref().delete::<tables::BytecodeChunks>(hashes[0], None).unwrap();
    assert!(matches!(writer.verify_completeness(write, 1, &cancel),
        Err(SnapSyncError::MissingCode { hash: missing }) if missing == hash));
    assert!(matches!(writer.start_trie_rebuild(write, 1, &cancel),
        Err(SnapSyncError::MissingCode { hash: missing }) if missing == hash));
    assert!(!writer.is_trie_rebuild_started(write).unwrap());
    writer.stage_chunked_code(write, hash, 49083, hashes, chunks).unwrap();
    writer.start_trie_rebuild(write, 1, &cancel).unwrap();
    assert!(writer.is_trie_rebuild_started(write).unwrap());
    writer.commit().unwrap();
}

#[test]
fn t022_ordinary_downloads_remain_unsplit() {
    let factory = create_test_provider_factory();
    let writer = factory.provider_rw().unwrap();
    writer.write_storage_settings(StorageSettings::v2()).unwrap();
    writer.commit().unwrap();
    factory.set_storage_settings_cache(StorageSettings::v2());
    insert_headers(&factory, &[SealedHeader::new(Header::default(), B256::ZERO)]);
    let writer = factory.provider_rw().unwrap();
    let write = writer
        .start_snap_attempt(SnapGeneration::new(BlockNumHash::new(0, B256::ZERO), B256::ZERO))
        .unwrap();
    // Deliberately not valid chunk boundaries: ordinary sync must not opt in.
    let bytes = Bytes::from(vec![0x01; 24542]);
    let hash = keccak256(&bytes);
    assert_eq!(writer.commit_bytecodes(write, vec![(hash, bytes.clone())]).unwrap(), 1);
    assert_eq!(writer.tx_ref().entries::<tables::BytecodeChunkDescriptors>().unwrap(), 0);
    assert_eq!(writer.tx_ref().entries::<tables::BytecodeChunks>().unwrap(), 0);
    writer.commit().unwrap();
    assert_eq!(
        factory.latest().unwrap().bytecode_by_hash(&hash).unwrap().unwrap().original_bytes(),
        bytes
    );
}

#[test]
fn typed_snap_accounts_publish_supplied_chunks_and_inline_delegation() {
    for delegated in [false, true] {
        let factory = create_test_provider_factory();
        let writer = factory.provider_rw().unwrap();
        writer.write_storage_settings(StorageSettings::v2()).unwrap();
        writer.commit().unwrap();
        factory.set_storage_settings_cache(StorageSettings::v2());
        let target = alloy_primitives::Address::repeat_byte(0x77);
        let mut raw = vec![0; 24542];
        raw[24540] = 0x60;
        raw[24541] = 0xa7;
        let original = Bytes::from(raw);
        let native = if delegated {
            evm2::evm::AccountInfo {
                nonce: 1,
                code_hash: evm2::bytecode::Bytecode::new_eip7702(target).hash_slow(),
                inline_delegation: Some(target),
                ..Default::default()
            }
        } else {
            evm2::evm::AccountInfo {
                nonce: 1,
                code_hash: keccak256(&original),
                code_metadata: evm2::bytecode::code_metadata(&original).unwrap(),
                ..Default::default()
            }
        };
        let account = Account::from(reth_execution_types::revm_account(&native));
        let key = B256::repeat_byte(0x22);
        let trie_account = account.clone().into_trie_account(EMPTY_ROOT_HASH);
        let root = state_root_unsorted([(key, trie_account.clone())]);
        let pivot = B256::repeat_byte(1);
        insert_headers(
            &factory,
            &[
                SealedHeader::new(Header::default(), B256::ZERO),
                SealedHeader::new(
                    Header { number: 1, state_root: root, ..Default::default() },
                    pivot,
                ),
            ],
        );
        let writer = factory.provider_rw().unwrap();
        let write = writer
            .start_snap_attempt(SnapGeneration::new(BlockNumHash::new(1, pivot), root))
            .unwrap();
        writer.start_account_coverage(write).unwrap();
        writer.commit().unwrap();
        let range = VerifiedAccountRange::verify_response(
            GetAccountRangeMessage {
                request_id: 1,
                root_hash: root,
                starting_hash: B256::ZERO,
                limit_hash: B256::repeat_byte(0xff),
                response_bytes: 1_000_000,
            },
            AccountRangeMessage {
                request_id: 1,
                accounts: vec![AccountData::from_trie_account(key, &trie_account)],
                proof: vec![],
            },
        )
        .unwrap();
        let supplied = if delegated {
            vec![]
        } else {
            vec![(native.code_hash, revm::state::Bytecode::new_legacy(original.clone()))]
        };
        let writer = factory.provider_rw().unwrap();
        writer.commit_account_range(write, &range, Default::default(), supplied).unwrap();
        writer.verify_completeness(write, 1, &CancellationToken::new()).unwrap();
        assert_eq!(writer.tx_ref().get::<tables::HashedAccounts>(key).unwrap(), Some(account));
        assert!(writer.tx_ref().get::<tables::Bytecodes>(native.code_hash).unwrap().is_none());
        if delegated {
            assert_eq!(writer.tx_ref().entries::<tables::BytecodeChunks>().unwrap(), 0);
        } else {
            let descriptor = writer
                .tx_ref()
                .get::<tables::BytecodeChunkDescriptors>(native.code_hash)
                .unwrap()
                .unwrap();
            assert_eq!(descriptor.preparation(1).unwrap().leading_data_len, 1);
            assert_eq!(descriptor.preparation(0).unwrap().lookahead.as_ref(), &[0xa7]);
        }
        writer.commit().unwrap();
    }
}

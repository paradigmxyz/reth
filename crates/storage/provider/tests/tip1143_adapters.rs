//! T-020 and T-021: actual EVM/revm and execution-cache adapters over persistence.
//!
//! Adapter inventory: references, Box, Arc, EvmStateProviderAdapter, EvmStateProviderBox,
//! both EVM/revm StateProviderDatabase types, InstrumentedStateProvider,
//! CachedStateProvider, and the txpool snapshot are exercised below. Resident block
//! overlays and indexed history are exercised in tip1143_state_views.rs.
//! The pinned storage-api, EVM, and revm crates expose synchronous state database
//! interfaces, not a separate async database wrapper. Async production consumers are
//! exercised through the RPC and snap handlers in tip1143_serving.rs; the remote
//! RPC provider's blocking-to-async transport bridge is exercised below.
//! New sparse forwarding methods are proposed production APIs, not local adapters.

use alloy_eips::BlockId;
use alloy_network::AnyNetwork;
use alloy_primitives::{keccak256, Address, Bytes, B256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_transport::mock::Asserter;
use reth_chainspec::MAINNET;
use reth_db::{
    init_db, mdbx::DatabaseArguments, open_db, test_utils::DatabaseTestHooks, DatabaseEnv,
};
use reth_db_api::{
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_engine_tree::tree::instrumented_state::InstrumentedStateProvider;
use reth_evm::cached::{AccountInfo, Bytecode as EvmBytecode, CachedReads};
use reth_execution_cache::{CachedStateProvider, ExecutionCache, TxPoolPrewarmCacheSnapshot};
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
    AccountReader, BytecodeReader, CodeChunkReader, CodeRepresentation, DBProvider,
    EvmStateProviderAdapter, EvmStateProviderBox, ValidatedCode,
};
use reth_storage_errors::provider::CodeChunkErrorKind;
use reth_storage_rpc_provider::RpcBlockchainStateProvider;
use std::{marker::PhantomData, path::Path, sync::Arc};

#[test]
fn t020_real_forwarding_adapters_preserve_chunk_contract() {
    let factory = create_test_provider_factory();
    let original = Bytes::from(vec![0; 24542]);
    let hash = keccak256(&original);
    let code = ValidatedCode::new(original).unwrap();
    let writer = factory.provider_rw().unwrap();
    writer
        .write_chunked_code(
            Address::ZERO,
            Account { bytecode_hash: Some(hash), ..Default::default() },
            &code,
        )
        .unwrap();
    writer.commit().unwrap();
    let state = factory.latest().unwrap();
    assert_payloads(&state, hash);
    assert_payloads(&&state, hash);
    assert_payloads(&Box::new(factory.latest().unwrap()), hash);
    assert_payloads(&Arc::new(factory.latest().unwrap()), hash);
    assert_payloads(&EvmStateProviderAdapter(factory.latest().unwrap()), hash);
    let erased: EvmStateProviderBox = Box::new(EvmStateProviderAdapter(factory.latest().unwrap()));
    assert_payloads(&erased, hash);
    let evm = reth_evm::database::StateProviderDatabase::new(EvmStateProviderAdapter(
        factory.latest().unwrap(),
    ));
    assert_payloads(&evm, hash);
    let revm = reth_revm::database::StateProviderDatabase::new(EvmStateProviderAdapter(
        factory.latest().unwrap(),
    ));
    assert_payloads(&revm, hash);
    drop(state);
    let writer = factory.provider_rw().unwrap();
    assert!(writer.tx_ref().delete::<tables::BytecodeChunks>(keccak256([0]), None).unwrap());
    writer.commit().unwrap();
    let representation = CodeRepresentation::Chunked(code.descriptor().unwrap().clone());
    let erased: EvmStateProviderBox = Box::new(EvmStateProviderAdapter(factory.latest().unwrap()));
    assert!(CodeChunkReader::get_required_code_chunk(&erased, &hash, &representation, 1).is_err());
}

#[test]
fn t021_failed_cache_fill_does_not_poison_fresh_snapshot_retry() {
    let factory = create_test_provider_factory();
    let original = Bytes::from(vec![0; 24542]);
    let hash = keccak256(&original);
    let code = ValidatedCode::new(original).unwrap();
    let writer = factory.provider_rw().unwrap();
    writer
        .write_chunked_code(
            Address::ZERO,
            Account { bytecode_hash: Some(hash), ..Default::default() },
            &code,
        )
        .unwrap();
    writer.commit().unwrap();
    let writer = factory.provider_rw().unwrap();
    assert!(writer.tx_ref().delete::<tables::BytecodeChunks>(keccak256([0]), None).unwrap());
    writer.commit().unwrap();
    let cache = ExecutionCache::new(1024 * 1024);
    let cached = CachedStateProvider::new_prewarm(factory.latest().unwrap(), cache.clone());
    assert!(CodeChunkReader::get_code_chunk_by_hash(&cached, &hash, 1).is_err());
    drop(cached);
    let writer = factory.provider_rw().unwrap();
    writer
        .tx_ref()
        .put::<tables::BytecodeChunks>(keccak256([0]), Bytes::from_static(&[0]))
        .unwrap();
    writer.commit().unwrap();
    let cached = CachedStateProvider::new_prewarm(factory.latest().unwrap(), cache);
    assert_payloads(&cached, hash);
    assert_payloads(&cached, hash);
}

fn assert_payloads(provider: &impl CodeChunkReader, hash: B256) {
    // UFCS and a trait bound prevent Deref from silently bypassing adapter forwarding.
    assert_eq!(
        CodeChunkReader::get_code_chunk_by_hash(provider, &hash, 0).unwrap(),
        Some(Bytes::from(vec![0; 24541]))
    );
    assert_eq!(
        CodeChunkReader::get_code_chunk_by_hash(provider, &hash, 1).unwrap(),
        Some(Bytes::from_static(&[0]))
    );
    assert_eq!(CodeChunkReader::get_code_chunk_by_hash(provider, &hash, 2).unwrap(), None);
    assert_eq!(CodeChunkReader::get_code_chunk_by_hash(provider, &hash, u32::MAX).unwrap(), None);
}

/// T-021: a retained cache must distinguish both code identity and chunk index.
#[test]
fn t021_distinct_hashes_and_indices_remain_sparse_through_retained_cache() {
    let hooks = DatabaseTestHooks::default();
    let factory = create_test_provider_factory_with_db_hooks(hooks.clone());
    let mut fixtures = Vec::new();
    for tag in [0x11, 0x22] {
        let mut bytes = vec![0; 49085];
        for (index, chunk) in bytes.chunks_mut(24541).enumerate() {
            chunk[0] = 0x60;
            chunk[1] = tag + index as u8;
        }
        let original = Bytes::from(bytes);
        let hash = keccak256(&original);
        let writer = factory.provider_rw().unwrap();
        writer
            .write_chunked_code(
                Address::repeat_byte(tag),
                Account { bytecode_hash: Some(hash), ..Default::default() },
                &ValidatedCode::new(original.clone()).unwrap(),
            )
            .unwrap();
        // A full-code row makes an eager fallback observable, even if its result is correct.
        writer
            .tx_ref()
            .put::<tables::Bytecodes>(hash, Bytecode::new_raw(original.clone()))
            .unwrap();
        writer.commit().unwrap();
        fixtures.push((hash, original));
    }
    hooks.clear_reads();
    let control = factory.provider().unwrap();
    control.tx_ref().get::<tables::Bytecodes>(fixtures[0].0).unwrap().unwrap();
    control
        .tx_ref()
        .get::<tables::BytecodeChunks>(keccak256(&fixtures[1].1[24541..49082]))
        .unwrap()
        .unwrap();
    assert_eq!(hooks.reads().iter().filter(|read| read.table == "Bytecodes").count(), 1);
    assert_eq!(hooks.reads().iter().filter(|read| read.table == "BytecodeChunks").count(), 1);
    drop(control);

    let cache = ExecutionCache::new(1024 * 1024);
    for pass in 0..2 {
        for (hash, original) in &fixtures {
            for index in [2, 0, 1, 2, 3, u32::MAX] {
                // Include provider and cache initialization in the observation window.
                hooks.clear_reads();
                let cached =
                    CachedStateProvider::new_prewarm(factory.latest().unwrap(), cache.clone());
                let actual = CodeChunkReader::get_code_chunk_by_hash(&cached, hash, index).unwrap();
                let reads = hooks.reads();
                assert!(!reads.iter().any(|read| read.table == "Bytecodes"));
                let payloads =
                    reads.iter().filter(|read| read.table == "BytecodeChunks").collect::<Vec<_>>();
                if index < 3 {
                    let start = index as usize * 24541;
                    let expected = &original[start..(start + 24541).min(original.len())];
                    assert_eq!(actual, Some(Bytes::copy_from_slice(expected)));
                    // Forwarding and a dedicated immutable chunk cache are both allowed.
                    assert!(payloads.len() <= 1);
                    if pass == 0 && index == 0 {
                        assert_eq!(payloads.len(), 1, "first access needs the original payload");
                    }
                    for read in payloads {
                        assert_eq!(read.key, keccak256(expected).as_slice());
                        assert_eq!(read.value.as_deref(), Some(expected));
                    }
                } else {
                    assert_eq!(actual, None);
                    assert!(payloads.is_empty());
                }
                for metadata in reads.iter().filter(|read| read.table == "BytecodeChunkDescriptors")
                {
                    assert!(metadata.value.as_ref().is_none_or(|value| value.len() <= 1285));
                }
            }
        }
    }
}

/// T-020: every concrete forwarding path must retain sparse I/O and required errors.
#[test]
fn t020_each_adapter_preserves_sparse_errors_and_full_code() {
    let hooks = DatabaseTestHooks::default();
    let factory = create_test_provider_factory_with_db_hooks(hooks.clone());
    let mut bytes = vec![0; 49083];
    bytes[0] = 0x60;
    bytes[1] = 0x11;
    bytes[24541] = 0x60;
    bytes[24542] = 0x22;
    bytes[49082] = 0x01;
    let original = Bytes::from(bytes);
    let hash = keccak256(&original);
    let code = ValidatedCode::new(original.clone()).unwrap();
    let representation = CodeRepresentation::Chunked(code.descriptor().unwrap().clone());
    let writer = factory.provider_rw().unwrap();
    writer
        .write_chunked_code(
            Address::ZERO,
            Account { bytecode_hash: Some(hash), ..Default::default() },
            &code,
        )
        .unwrap();
    // A valid whole-code decoy must not mask the missing final payload.
    writer.tx_ref().put::<tables::Bytecodes>(hash, Bytecode::new_raw(original.clone())).unwrap();
    writer.commit().unwrap();
    // Observe the actual routes with a positive control before testing forwarding.
    hooks.clear_reads();
    let reader = factory.provider().unwrap();
    assert!(reader.tx_ref().get::<tables::BytecodeChunks>(keccak256([1])).unwrap().is_some());
    assert!(reader.tx_ref().get::<tables::Bytecodes>(hash).unwrap().is_some());
    assert_eq!(hooks.reads().iter().filter(|r| r.table == "BytecodeChunks").count(), 1);
    assert_eq!(hooks.reads().iter().filter(|r| r.table == "Bytecodes").count(), 1);
    drop(reader);

    for corrupt in [false, true] {
        if corrupt {
            let writer = factory.provider_rw().unwrap();
            assert!(writer
                .tx_ref()
                .delete::<tables::BytecodeChunks>(keccak256([1]), None)
                .unwrap());
            writer.commit().unwrap();
        }
        // Clear before construction so eager initialization is also observable.
        hooks.clear_reads();
        let state = factory.latest().unwrap();
        assert_adapter_contract(&state, hash, &representation, &original, corrupt, &hooks);
        hooks.clear_reads();
        assert_adapter_contract(&&state, hash, &representation, &original, corrupt, &hooks);
        hooks.clear_reads();
        assert_adapter_contract(
            &Box::new(factory.latest().unwrap()),
            hash,
            &representation,
            &original,
            corrupt,
            &hooks,
        );
        hooks.clear_reads();
        assert_adapter_contract(
            &Arc::new(factory.latest().unwrap()),
            hash,
            &representation,
            &original,
            corrupt,
            &hooks,
        );
        hooks.clear_reads();
        assert_adapter_contract(
            &EvmStateProviderAdapter(factory.latest().unwrap()),
            hash,
            &representation,
            &original,
            corrupt,
            &hooks,
        );
        hooks.clear_reads();
        let erased: EvmStateProviderBox =
            Box::new(EvmStateProviderAdapter(factory.latest().unwrap()));
        assert_adapter_contract(&erased, hash, &representation, &original, corrupt, &hooks);
        hooks.clear_reads();
        let evm = reth_evm::database::StateProviderDatabase::new(EvmStateProviderAdapter(
            factory.latest().unwrap(),
        ));
        assert_adapter_contract(&evm, hash, &representation, &original, corrupt, &hooks);
        hooks.clear_reads();
        let revm = reth_revm::database::StateProviderDatabase::new(EvmStateProviderAdapter(
            factory.latest().unwrap(),
        ));
        assert_adapter_contract(&revm, hash, &representation, &original, corrupt, &hooks);
    }
}

fn assert_adapter_contract(
    provider: &(impl CodeChunkReader + BytecodeReader),
    hash: B256,
    representation: &CodeRepresentation,
    original: &Bytes,
    corrupt: bool,
    hooks: &DatabaseTestHooks,
) {
    assert_eq!(
        CodeChunkReader::get_required_code_chunk(provider, &hash, representation, 1)
            .unwrap()
            .unwrap()
            .as_ref(),
        &original[24541..49082]
    );
    let reads = hooks.reads();
    assert!(!reads.iter().any(|r| r.table == "Bytecodes"));
    let payloads = reads.iter().filter(|r| r.table == "BytecodeChunks").collect::<Vec<_>>();
    assert_eq!(payloads.len(), 1);
    assert_eq!(payloads[0].key, keccak256(&original[24541..49082]).as_slice());
    assert_eq!(payloads[0].value.as_deref(), Some(&original[24541..49082]));
    hooks.clear_reads();
    for index in [3, u32::MAX] {
        assert_eq!(
            CodeChunkReader::get_required_code_chunk(provider, &hash, representation, index)
                .unwrap(),
            None
        );
    }
    assert!(hooks.reads().is_empty());
    let final_chunk = CodeChunkReader::get_required_code_chunk(provider, &hash, representation, 2);
    if corrupt {
        let ProviderError::CodeChunk(error) = final_chunk.unwrap_err() else {
            panic!("adapter lost required chunk error")
        };
        assert_eq!(error.code_hash, hash);
        assert_eq!(error.index, 2);
        assert_eq!(error.expected_length, Some(1));
        assert_eq!(error.reason, CodeChunkErrorKind::MissingPayload);
    } else {
        assert_eq!(final_chunk.unwrap(), Some(Bytes::from_static(&[1])));
    }
    assert!(!hooks.reads().iter().any(|r| r.table == "Bytecodes"));
    let full = BytecodeReader::bytecode_by_hash(provider, &hash);
    if corrupt {
        assert!(full.is_err());
    } else {
        assert_eq!(full.unwrap().unwrap().original_bytes(), *original);
    }
    hooks.clear_reads();
    assert_eq!(
        CodeChunkReader::get_code_chunk_by_hash(provider, &B256::repeat_byte(0x99), 0).unwrap(),
        None
    );
}

/// T-020: exercise the engine's actual latency/counting adapter over MDBX.
#[test]
fn t020_engine_instrumentation_preserves_sparse_reads_and_errors() {
    let hooks = DatabaseTestHooks::default();
    let factory = create_test_provider_factory_with_db_hooks(hooks.clone());
    let original = Bytes::from(vec![0; 24542]);
    let hash = keccak256(&original);
    let code = ValidatedCode::new(original.clone()).unwrap();
    let representation = CodeRepresentation::Chunked(code.descriptor().unwrap().clone());
    let writer = factory.provider_rw().unwrap();
    writer
        .write_chunked_code(
            Address::ZERO,
            Account { bytecode_hash: Some(hash), ..Default::default() },
            &code,
        )
        .unwrap();
    writer.tx_ref().put::<tables::Bytecodes>(hash, Bytecode::new_raw(original)).unwrap();
    writer.commit().unwrap();
    for missing in [false, true] {
        if missing {
            let writer = factory.provider_rw().unwrap();
            assert!(writer
                .tx_ref()
                .delete::<tables::BytecodeChunks>(keccak256([0]), None)
                .unwrap());
            writer.commit().unwrap();
        }
        hooks.clear_reads();
        let instrumented = InstrumentedStateProvider::new(
            EvmStateProviderAdapter(factory.latest().unwrap()),
            "tip1143_acceptance",
        );
        let result =
            CodeChunkReader::get_required_code_chunk(&instrumented, &hash, &representation, 1);
        if missing {
            let ProviderError::CodeChunk(error) = result.unwrap_err() else {
                panic!("instrumented read lost the chunk diagnostic");
            };
            assert_eq!(error.code_hash, hash);
            assert_eq!(error.index, 1);
            assert_eq!(error.expected_length, Some(1));
            assert_eq!(error.reason, CodeChunkErrorKind::MissingPayload);
        } else {
            assert_eq!(result.unwrap(), Some(Bytes::from_static(&[0])));
        }
        assert_eq!(hooks.reads().iter().filter(|r| r.table == "BytecodeChunks").count(), 1);
        assert!(!hooks.reads().iter().any(|r| r.table == "Bytecodes"));
        hooks.clear_reads();
        assert_eq!(
            CodeChunkReader::get_required_code_chunk(
                &instrumented,
                &hash,
                &representation,
                u32::MAX
            )
            .unwrap(),
            None
        );
        assert!(hooks.reads().is_empty());
    }
}

/// T-021: the actual txpool snapshot wins over a different valid persisted identity.
#[test]
fn t021_txpool_snapshot_resident_code_precedes_persistent_state() {
    let hooks = DatabaseTestHooks::default();
    let factory = create_test_provider_factory_with_db_hooks(hooks.clone());
    let owner = Address::repeat_byte(0x21);
    let persisted = Bytes::from_static(&[0x01]);
    let writer = factory.provider_rw().unwrap();
    writer
        .write_chunked_code(
            owner,
            Account { nonce: 1, bytecode_hash: Some(keccak256(&persisted)), ..Default::default() },
            &ValidatedCode::new(persisted).unwrap(),
        )
        .unwrap();
    writer.commit().unwrap();
    let mut original = vec![0; 49083];
    original[0] = 0x60;
    original[1] = 0x21;
    original[24541] = 0x60;
    original[24542] = 0x43;
    original[49082] = 0x5b;
    let original = Bytes::from(original);
    let hash = keccak256(&original);
    ValidatedCode::new(original.clone()).unwrap();
    let mut reads = CachedReads::default();
    reads.insert_account(
        owner,
        AccountInfo { nonce: 2, code_hash: hash, ..Default::default() },
        Default::default(),
    );
    reads.contracts.insert(hash, EvmBytecode::new_legacy(original.clone()));
    let snapshot = TxPoolPrewarmCacheSnapshot::new(B256::repeat_byte(0x21), Arc::new(reads));
    assert_eq!(snapshot.entry_counts(), (1, 0, 1));
    hooks.clear_reads();
    let cached = CachedStateProvider::new_prewarm(
        factory.latest().unwrap(),
        ExecutionCache::new(4 * 1024 * 1024),
    )
    .with_txpool_snapshot(Some(snapshot));
    let account = cached.basic_account(&owner).unwrap().unwrap();
    assert_eq!(account.nonce, 2);
    assert_eq!(account.bytecode_hash, Some(hash));
    // Resolve the identity from the real snapshot account before requesting payloads.
    let resolved = account.bytecode_hash.unwrap();
    for index in [2, 0, 1, 2] {
        let start = index as usize * 24541;
        assert_eq!(
            CodeChunkReader::get_code_chunk_by_hash(&cached, &resolved, index)
                .unwrap()
                .unwrap()
                .as_ref(),
            &original[start..(start + 24541).min(original.len())]
        );
    }
    assert_eq!(CodeChunkReader::get_code_chunk_by_hash(&cached, &resolved, 3).unwrap(), None);
    assert_eq!(cached.bytecode_by_hash(&resolved).unwrap().unwrap().original_bytes(), original);
    assert!(!hooks.reads().iter().any(|r| r.table == "Bytecodes" || r.table == "BytecodeChunks"));
}

/// T-020/T-023: a remote provider without a chunk endpoint refuses sparse reads.
/// The transport queue detects attempted requests; it never fabricates code responses.
#[tokio::test(flavor = "multi_thread")]
async fn t020_remote_sparse_capability_does_not_download_whole_code() {
    let transport = Asserter::new();
    let rpc =
        ProviderBuilder::new().network::<AnyNetwork>().connect_mocked_client(transport.clone());
    transport.push_failure_msg("positive control");
    assert!(rpc.get_block_number().await.is_err());
    assert!(transport.read_q().is_empty());
    transport.push_failure_msg("unexpected sparse fallback request");
    let state = RpcBlockchainStateProvider::<_, MockNodeTypesWithDB, AnyNetwork>::new(
        rpc.clone(),
        BlockId::latest(),
        PhantomData,
    );
    assert_eq!(transport.read_q().len(), 1);
    let hash = keccak256(b"remote chunk identity");
    for index in [0, 1, u32::MAX] {
        assert!(matches!(
            state.get_code_chunk_by_hash(&hash, index),
            Err(ProviderError::UnsupportedProvider)
        ));
        assert_eq!(transport.read_q().len(), 1);
    }
    assert!(matches!(
        state.get_required_code_chunk(&hash, &CodeRepresentation::Legacy, 0),
        Err(ProviderError::UnsupportedProvider)
    ));
    assert_eq!(transport.read_q().len(), 1);
    // Prove the same retained transport still consumes responses for actual RPC calls.
    assert!(rpc.get_block_number().await.is_err());
    assert!(transport.read_q().is_empty());
}

/// T-013/T-020: real EVM conversions preserve kind and original content.
#[test]
fn t013_ef01_bytecode_kind_through_evm_adapters() {
    for multi in [false, true] {
        for delegation in [false, true] {
            if multi && delegation {
                continue;
            }
            let factory = create_test_provider_factory();
            let mut original = vec![0; if multi { 24564 } else { 23 }];
            original[..3].copy_from_slice(&[0xef, 0x01, 0x00]);
            if multi {
                original[24541..24544].copy_from_slice(&[0xef, 0x01, 0x00]);
                original[24544..].fill(0x11);
            } else {
                original[3..].fill(0x11);
            }
            let original = Bytes::from(original);
            let hash = keccak256(&original);
            let writer = factory.provider_rw().unwrap();
            if multi {
                writer
                    .write_chunked_code(
                        Address::ZERO,
                        Account { bytecode_hash: Some(hash), ..Default::default() },
                        &ValidatedCode::new(original.clone()).unwrap(),
                    )
                    .unwrap();
            } else {
                let code = Bytecode(if delegation {
                    revm::bytecode::Bytecode::new_eip7702(Address::repeat_byte(0x11))
                } else {
                    revm::bytecode::Bytecode::new_legacy(original.clone())
                });
                writer.tx_ref().put::<tables::Bytecodes>(hash, code).unwrap();
            }
            writer.commit().unwrap();
            let mut native = reth_evm::database::StateProviderDatabase::new(
                EvmStateProviderAdapter(factory.latest().unwrap()),
            );
            let code = reth_evm::Database::get_code_by_hash(&mut native, &hash).unwrap();
            assert_eq!(code.original_bytes(), original);
            assert_eq!(code.is_eip7702(), delegation);
            assert_eq!(code.eip7702_address(), delegation.then_some(Address::repeat_byte(0x11)));
            let mut database = reth_revm::database::StateProviderDatabase::new(
                EvmStateProviderAdapter(factory.latest().unwrap()),
            );
            let code = revm::Database::code_by_hash(&mut database, hash).unwrap();
            assert_eq!(code.original_bytes(), original);
            assert_eq!(code.is_legacy(), !delegation);
            assert_eq!(code.is_eip7702(), delegation);
            if multi {
                // Sparse interfaces own raw payloads; they must not classify this marker.
                assert_eq!(
                    database.get_code_chunk_by_hash(&hash, 1).unwrap().unwrap().as_ref(),
                    &original[24541..]
                );
            }
        }
    }
}

/// T-020: no adapter may hide eager reads in factory initialization or writer caches.
#[test]
fn t020_cold_factory_observation_includes_adapter_initialization() {
    let directory = tempfile::tempdir().unwrap();
    let hooks = DatabaseTestHooks::default();
    let factory = open_observed_factory(directory.path(), true, hooks.clone());
    let mut original = vec![0; 49083];
    original[0] = 1;
    original[24541] = 2;
    original[49082] = 1;
    let original = Bytes::from(original);
    let hash = keccak256(&original);
    let code = ValidatedCode::new(original.clone()).unwrap();
    let representation = CodeRepresentation::Chunked(code.descriptor().unwrap().clone());
    let writer = factory.provider_rw().unwrap();
    writer
        .write_chunked_code(
            Address::ZERO,
            Account { bytecode_hash: Some(hash), ..Default::default() },
            &code,
        )
        .unwrap();
    writer.commit().unwrap();
    drop(factory);
    for adapter in 0..8 {
        hooks.clear_reads();
        let factory = open_observed_factory(directory.path(), false, hooks.clone());
        let state = factory.latest().unwrap();
        match adapter {
            0 => assert_adapter_contract(&state, hash, &representation, &original, false, &hooks),
            1 => assert_adapter_contract(&&state, hash, &representation, &original, false, &hooks),
            2 => assert_adapter_contract(
                &Box::new(state),
                hash,
                &representation,
                &original,
                false,
                &hooks,
            ),
            3 => assert_adapter_contract(
                &Arc::new(state),
                hash,
                &representation,
                &original,
                false,
                &hooks,
            ),
            4 => assert_adapter_contract(
                &EvmStateProviderAdapter(state),
                hash,
                &representation,
                &original,
                false,
                &hooks,
            ),
            5 => {
                let erased: EvmStateProviderBox = Box::new(EvmStateProviderAdapter(state));
                assert_adapter_contract(&erased, hash, &representation, &original, false, &hooks);
            }
            6 => assert_adapter_contract(
                &reth_evm::database::StateProviderDatabase::new(EvmStateProviderAdapter(state)),
                hash,
                &representation,
                &original,
                false,
                &hooks,
            ),
            7 => assert_adapter_contract(
                &reth_revm::database::StateProviderDatabase::new(EvmStateProviderAdapter(state)),
                hash,
                &representation,
                &original,
                false,
                &hooks,
            ),
            _ => unreachable!(),
        }
    }
}

/// The proposed hook observes real storage beginning with the environment open.
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

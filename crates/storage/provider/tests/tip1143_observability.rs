//! T-025: structured diagnostics survive the production EVM adapter.
//!
//! Uses the proposed chunk API and errors shared with tip1143_corruption.rs.
//! These callers have no block/transaction context; none is invented by this test.
//! Raw malformed descriptor decoding must retain hash/index even when length is unknown.

use alloy_primitives::{keccak256, Address, Bytes};
use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
use reth_db::test_utils::DatabaseTestHooks;
use reth_db_api::{
    tables::{self, RawTable, RawValue},
    transaction::{DbTx, DbTxMut},
};
use reth_execution_cache::{CachedStateProvider, ExecutionCache};
use reth_primitives_traits::Account;
use reth_provider::{
    test_utils::create_test_provider_factory_with_db_hooks, ProviderError, StateProviderFactory,
};
use reth_storage_api::{
    CodeChunkReader, CodeReadContext, CodeRepresentation, DBProvider, EvmStateProviderAdapter,
    ValidatedCode,
};
use reth_storage_errors::{
    db::{DatabaseError, DatabaseErrorInfo},
    provider::CodeChunkErrorKind,
};
use std::{cell::RefCell, collections::HashMap};

#[test]
fn t025_missing_and_wrong_length_diagnostics_survive_evm_forwarding() {
    for index in 0..3usize {
        let expected_length = if index == 2 { 7 } else { 24541 };
        for actual in [None, Some(0), Some(expected_length - 1), Some(expected_length + 1)] {
            let hooks = DatabaseTestHooks::default();
            let factory = create_test_provider_factory_with_db_hooks(hooks.clone());
            let mut original = vec![0; 49089];
            for (tag, chunk) in original.chunks_mut(24541).enumerate() {
                chunk[0] = 0x60;
                chunk[1] = tag as u8;
            }
            let hash = keccak256(&original);
            let payload_hash =
                keccak256(&original[index * 24541..(index * 24541 + expected_length)]);
            let code = ValidatedCode::new(original.into()).unwrap();
            let representation = CodeRepresentation::Chunked(code.descriptor().unwrap().clone());
            let writer = factory.provider_rw().unwrap();
            writer
                .write_chunked_code(
                    Address::repeat_byte(0x25),
                    Account { bytecode_hash: Some(hash), ..Default::default() },
                    &code,
                )
                .unwrap();
            writer.commit().unwrap();
            // The successful request is a positive control for both adapter and observer.
            hooks.clear_reads();
            let adapter = EvmStateProviderAdapter(factory.latest().unwrap());
            assert_eq!(
                CodeChunkReader::get_required_code_chunk(
                    &adapter,
                    &hash,
                    &representation,
                    index as u32,
                )
                .unwrap()
                .unwrap()
                .len(),
                expected_length
            );
            assert_eq!(
                hooks.reads().iter().filter(|read| read.table == "BytecodeChunks").count(),
                1
            );
            drop(adapter);
            let writer = factory.provider_rw().unwrap();
            if let Some(length) = actual {
                writer
                    .tx_ref()
                    .put::<tables::BytecodeChunks>(payload_hash, Bytes::from(vec![0; length]))
                    .unwrap();
            } else {
                assert!(writer
                    .tx_ref()
                    .delete::<tables::BytecodeChunks>(payload_hash, None)
                    .unwrap());
            }
            assert_eq!(
                writer.tx_ref().get::<tables::BytecodeChunks>(payload_hash).unwrap(),
                actual.map(|length| Bytes::from(vec![0; length]))
            );
            writer.commit().unwrap();
            hooks.clear_reads();
            let adapter = EvmStateProviderAdapter(factory.latest().unwrap());
            let error = CodeChunkReader::get_required_code_chunk(
                &adapter,
                &hash,
                &representation,
                index as u32,
            )
            .unwrap_err();
            let ProviderError::CodeChunk(error) = error else {
                panic!("lost chunk diagnostic through EVM adapter: {error:?}")
            };
            assert_eq!(error.code_hash, hash);
            assert_eq!(error.index, index as u32);
            assert_eq!(error.expected_length, Some(expected_length));
            assert_eq!(
                error.reason,
                actual.map_or(CodeChunkErrorKind::MissingPayload, |actual| {
                    CodeChunkErrorKind::InvalidLength { actual }
                })
            );
            assert!(!hooks.reads().iter().any(|read| read.table == "Bytecodes"));
        }
    }
}

#[test]
fn t025_malformed_descriptor_reports_unknown_length_and_requested_identity() {
    for malformed in [vec![], vec![0], vec![0, 0, 0x60]] {
        let hooks = DatabaseTestHooks::default();
        let factory = create_test_provider_factory_with_db_hooks(hooks.clone());
        let hash = keccak256(b"T-025 malformed descriptor identity");
        let writer = factory.provider_rw().unwrap();
        writer
            .tx_ref()
            .put::<RawTable<tables::BytecodeChunkDescriptors>>(
                hash.into(),
                RawValue::from_vec(malformed.clone()),
            )
            .unwrap();
        writer.commit().unwrap();
        let reader = factory.provider().unwrap();
        assert_eq!(
            reader
                .tx_ref()
                .get::<RawTable<tables::BytecodeChunkDescriptors>>(hash.into())
                .unwrap()
                .unwrap()
                .raw_value(),
            malformed.as_slice()
        );
        drop(reader);
        hooks.clear_reads();
        let adapter = EvmStateProviderAdapter(factory.latest().unwrap());
        let error = CodeChunkReader::get_code_chunk_by_hash(&adapter, &hash, 1).unwrap_err();
        let ProviderError::CodeChunk(error) = error else {
            panic!("descriptor error lost requested identity: {error:?}")
        };
        assert_eq!(error.code_hash, hash);
        assert_eq!(error.index, 1);
        assert_eq!(error.expected_length, None);
        // Proposed structured category; decoder text is not used as the oracle.
        assert_eq!(error.reason, CodeChunkErrorKind::MalformedDescriptor);
        assert!(!hooks
            .reads()
            .iter()
            .any(|read| matches!(read.table.as_str(), "Bytecodes" | "BytecodeChunks")));
    }
}

/// T-026 proposes byte and latency metrics for physical payload read attempts.
/// Successful reads count original payload bytes, including malformed payloads.
/// Missing records count zero bytes but still record a latency sample.
/// Bounds shortcuts record neither bytes nor physical-read latency.
#[test]
fn t026_physical_metrics_match_observed_reads() {
    let recorder = DebuggingRecorder::new();
    let snapshotter = MetricTotals { inner: recorder.snapshotter(), totals: RefCell::default() };
    // Thread-local installation isolates this case from other integration tests.
    metrics::with_local_recorder(&recorder, || {
        metrics::counter!("tip1143.test.recorder_control").increment(7);
        assert_eq!(metric_count(&snapshotter, "tip1143.test.recorder_control"), 7);
        assert_eq!(metric_count(&snapshotter, "tip1143.test.recorder_control"), 7);
        metrics::counter!("tip1143.test.recorder_control").increment(2);
        assert_eq!(metric_count(&snapshotter, "tip1143.test.recorder_control"), 9);
        let hooks = DatabaseTestHooks::default();
        let factory = create_test_provider_factory_with_db_hooks(hooks.clone());
        let mut original = vec![0; 24548];
        original[24541] = 0x01;
        let hash = keccak256(&original);
        let final_hash = keccak256(&original[24541..]);
        let code = ValidatedCode::new(original.clone().into()).unwrap();
        let writer = factory.provider_rw().unwrap();
        writer
            .write_chunked_code(
                Address::repeat_byte(0x26),
                Account { bytecode_hash: Some(hash), ..Default::default() },
                &code,
            )
            .unwrap();
        writer.commit().unwrap();
        for index in [0, 0, 1, 2, u32::MAX] {
            let before_bytes = metric_count(&snapshotter, "provider.code_chunks.bytes_fetched");
            let before_samples = metric_count(&snapshotter, "provider.code_chunks.read_latency");
            hooks.clear_reads();
            let state = factory.latest().unwrap();
            let actual = state.get_code_chunk_by_hash(&hash, index).unwrap();
            let expected = match index {
                0 => Some(Bytes::copy_from_slice(&original[..24541])),
                1 => Some(Bytes::copy_from_slice(&original[24541..])),
                _ => None,
            };
            assert_eq!(actual, expected);
            let reads = hooks.reads();
            let payloads =
                reads.iter().filter(|read| read.table == "BytecodeChunks").collect::<Vec<_>>();
            let fetched = payloads
                .iter()
                .map(|read| read.value.as_ref().map_or(0, Vec::len) as u64)
                .sum::<u64>();
            assert_eq!(payloads.len(), usize::from(index < 2));
            assert_eq!(
                metric_count(&snapshotter, "provider.code_chunks.bytes_fetched") - before_bytes,
                fetched
            );
            assert_eq!(
                metric_count(&snapshotter, "provider.code_chunks.read_latency") - before_samples,
                payloads.len() as u64
            );
        }
        // Fresh providers prevent authenticated resident bytes from masking corruption.
        for payload in [Some(Bytes::from_static(&[1])), None] {
            let writer = factory.provider_rw().unwrap();
            if let Some(bytes) = &payload {
                writer.tx_ref().put::<tables::BytecodeChunks>(final_hash, bytes.clone()).unwrap();
            } else {
                assert!(writer
                    .tx_ref()
                    .delete::<tables::BytecodeChunks>(final_hash, None)
                    .unwrap());
            }
            writer.commit().unwrap();
            let before_bytes = metric_count(&snapshotter, "provider.code_chunks.bytes_fetched");
            let before_samples = metric_count(&snapshotter, "provider.code_chunks.read_latency");
            hooks.clear_reads();
            let state = factory.latest().unwrap();
            assert!(state.get_code_chunk_by_hash(&hash, 1).is_err());
            let reads = hooks.reads();
            let payloads =
                reads.iter().filter(|read| read.table == "BytecodeChunks").collect::<Vec<_>>();
            assert_eq!(payloads.len(), 1);
            assert_eq!(payloads[0].value.as_deref(), payload.as_ref().map(|bytes| bytes.as_ref()));
            assert_eq!(
                metric_count(&snapshotter, "provider.code_chunks.bytes_fetched") - before_bytes,
                payload.as_ref().map_or(0, |bytes| bytes.len()) as u64
            );
            assert_eq!(
                metric_count(&snapshotter, "provider.code_chunks.read_latency") - before_samples,
                1
            );
        }
        let metrics = snapshotter.inner.snapshot().into_vec();
        let chunk_metrics = metrics
            .iter()
            .filter(|(key, _, _, _)| key.key().name().starts_with("provider.code_chunks."))
            .collect::<Vec<_>>();
        assert!(chunk_metrics.len() >= 2);
        for (key, _, _, _) in chunk_metrics {
            // These database aggregates need no identity labels at all.
            assert_eq!(key.key().labels().count(), 0);
        }
    });
}

/// The debug recorder drains every metric on snapshot, not just the requested one.
struct MetricTotals {
    inner: Snapshotter,
    totals: RefCell<HashMap<String, u64>>,
}

fn metric_count(snapshotter: &MetricTotals, name: &str) -> u64 {
    let mut totals = snapshotter.totals.borrow_mut();
    for (key, _, _, value) in snapshotter.inner.snapshot().into_vec() {
        let count = match value {
            DebugValue::Counter(value) => value,
            DebugValue::Histogram(values) => values.len() as u64,
            DebugValue::Gauge(_) => {
                assert_ne!(key.key().name(), name, "unexpected gauge for {name}");
                continue;
            }
        };
        *totals.entry(key.key().name().to_owned()).or_default() += count;
    }
    totals.get(name).copied().unwrap_or_default()
}

/// T-025 proposed context-aware required-read method and database read fault seam.
/// The hook fails an actual payload-table read, leaving metadata and other I/O real.
#[test]
fn t025_database_failure_retains_context_and_retry_succeeds() {
    for contextual in [false, true] {
        let hooks = DatabaseTestHooks::default();
        let factory = create_test_provider_factory_with_db_hooks(hooks.clone());
        let bytes = Bytes::from(vec![0; 24542]);
        let hash = keccak256(&bytes);
        let code = ValidatedCode::new(bytes).unwrap();
        let representation = CodeRepresentation::Chunked(code.descriptor().unwrap().clone());
        let writer = factory.provider_rw().unwrap();
        writer
            .write_chunked_code(
                Address::repeat_byte(0x25),
                Account { bytecode_hash: Some(hash), ..Default::default() },
                &code,
            )
            .unwrap();
        writer.commit().unwrap();
        let context = contextual.then_some(CodeReadContext {
            block_hash: Some(keccak256(b"T-025 block")),
            transaction_hash: Some(keccak256(b"T-025 transaction")),
        });
        let cause =
            DatabaseErrorInfo { message: "T-025 injected payload read".into(), code: -1143 };
        hooks.fail_next_table_read("BytecodeChunks", DatabaseError::Read(cause.clone()));
        let adapter = EvmStateProviderAdapter(factory.latest().unwrap());
        let error = adapter
            .get_required_code_chunk_with_context(&hash, &representation, 1, context.clone())
            .unwrap_err();
        assert_eq!(hooks.injected_failures(), 1);
        let ProviderError::CodeChunk(error) = error else {
            panic!("database failure lost chunk identity: {error:?}")
        };
        assert_eq!(error.code_hash, hash);
        assert_eq!(error.index, 1);
        assert_eq!(error.expected_length, Some(1));
        assert_eq!(error.context, context);
        let CodeChunkErrorKind::Database(DatabaseError::Read(actual)) = error.reason else {
            panic!("database failure lost its original cause: {:?}", error.reason)
        };
        assert_eq!(actual, cause);
        drop(adapter);
        hooks.disable_read_failure();
        hooks.clear_reads();
        let adapter = EvmStateProviderAdapter(factory.latest().unwrap());
        assert_eq!(
            adapter
                .get_required_code_chunk_with_context(&hash, &representation, 1, context)
                .unwrap(),
            Some(Bytes::from_static(&[0]))
        );
        assert_eq!(hooks.reads().iter().filter(|read| read.table == "BytecodeChunks").count(), 1);
        drop(adapter);
        let writer = factory.provider_rw().unwrap();
        assert!(writer.tx_ref().delete::<tables::BytecodeChunks>(keccak256([0]), None).unwrap());
        writer.commit().unwrap();
        let context = contextual.then_some(CodeReadContext {
            block_hash: Some(keccak256(b"T-025 block")),
            transaction_hash: Some(keccak256(b"T-025 transaction")),
        });
        let adapter = EvmStateProviderAdapter(factory.latest().unwrap());
        let error = adapter
            .get_required_code_chunk_with_context(&hash, &representation, 1, context.clone())
            .unwrap_err();
        let ProviderError::CodeChunk(error) = error else {
            panic!("missing payload lost contextual diagnostic: {error:?}")
        };
        assert_eq!(error.code_hash, hash);
        assert_eq!(error.index, 1);
        assert_eq!(error.expected_length, Some(1));
        assert_eq!(error.context, context);
        assert_eq!(error.reason, CodeChunkErrorKind::MissingPayload);
    }
}

/// T-026: cache counters match observed persistence, allowing forwarding-only caches.
#[test]
fn t026_cache_metrics_follow_physical_residency_without_requiring_chunk_caching() {
    let recorder = DebuggingRecorder::new();
    let snapshotter = MetricTotals { inner: recorder.snapshotter(), totals: RefCell::default() };
    metrics::with_local_recorder(&recorder, || {
        metrics::counter!("tip1143.test.cache_control").increment(1);
        assert_eq!(metric_count(&snapshotter, "tip1143.test.cache_control"), 1);
        let hooks = DatabaseTestHooks::default();
        let factory = create_test_provider_factory_with_db_hooks(hooks.clone());
        let bytes = Bytes::from(vec![0; 24542]);
        let hash = keccak256(&bytes);
        let writer = factory.provider_rw().unwrap();
        writer
            .write_chunked_code(
                Address::repeat_byte(0x26),
                Account { bytecode_hash: Some(hash), ..Default::default() },
                &ValidatedCode::new(bytes.clone()).unwrap(),
            )
            .unwrap();
        writer.commit().unwrap();
        let cache = ExecutionCache::new(1024 * 1024);
        for (pass, index) in [0, 0, 1, 1, 2, u32::MAX].into_iter().enumerate() {
            let hits = metric_count(&snapshotter, "execution_cache.code_chunks.hits");
            let misses = metric_count(&snapshotter, "execution_cache.code_chunks.misses");
            let fetched = metric_count(&snapshotter, "provider.code_chunks.bytes_fetched");
            hooks.clear_reads();
            let state = CachedStateProvider::new_prewarm(factory.latest().unwrap(), cache.clone());
            let result = state.get_code_chunk_by_hash(&hash, index).unwrap();
            let expected = match index {
                0 => Some(Bytes::copy_from_slice(&bytes[..24541])),
                1 => Some(Bytes::copy_from_slice(&bytes[24541..])),
                _ => None,
            };
            assert_eq!(result, expected);
            let reads = hooks.reads();
            let payloads =
                reads.iter().filter(|read| read.table == "BytecodeChunks").collect::<Vec<_>>();
            assert!(!reads.iter().any(|read| read.table == "Bytecodes"));
            if index < 2 {
                assert!(payloads.len() <= 1);
                if pass == 0 {
                    assert_eq!(payloads.len(), 1);
                }
                assert_eq!(
                    metric_count(&snapshotter, "execution_cache.code_chunks.misses") - misses,
                    payloads.len() as u64
                );
                assert_eq!(
                    metric_count(&snapshotter, "execution_cache.code_chunks.hits") - hits,
                    u64::from(payloads.is_empty())
                );
            } else {
                assert!(payloads.is_empty());
                assert_eq!(metric_count(&snapshotter, "execution_cache.code_chunks.hits"), hits);
                assert_eq!(
                    metric_count(&snapshotter, "execution_cache.code_chunks.misses"),
                    misses
                );
            }
            assert_eq!(
                metric_count(&snapshotter, "provider.code_chunks.bytes_fetched") - fetched,
                payloads
                    .iter()
                    .map(|read| read.value.as_ref().map_or(0, Vec::len) as u64)
                    .sum::<u64>()
            );
        }
        for (key, _, _, _) in snapshotter.inner.snapshot().into_vec() {
            if key.key().name().starts_with("execution_cache.code_chunks.") {
                assert_eq!(key.key().labels().count(), 0);
            }
        }
    });
}

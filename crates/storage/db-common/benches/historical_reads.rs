//! Benchmarks historical-state reads across Reth storage layouts.

#![allow(missing_docs)]

use std::{cell::Cell, hint::black_box, time::Duration};

use alloy_primitives::{Address, B256, U256};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use reth_chain_state::test_utils::TestBlockBuilder;
use reth_db_api::models::StorageSettings;
use reth_db_common::init::init_genesis_with_settings;
use reth_provider::{
    test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
    AccountReader, BlockHashReader, ChangeSetReader, DBProvider, DatabaseProviderFactory,
    ProviderFactory, SaveBlocksInput, StorageChangeSetReader, StorageSettingsCache,
};

const HISTORY_BLOCKS: u64 = 128;
const STORAGE_ADDRESS: Address = Address::new([0xAA; 20]);
const STORAGE_SLOT: B256 = B256::new(U256::from_limbs([1, 0, 0, 0]).to_be_bytes());

struct HistoricalReadFixture {
    factory: ProviderFactory<MockNodeTypesWithDB>,
    signer: Address,
    account_query_blocks: Vec<u64>,
    storage_query_blocks: Vec<u64>,
}

impl HistoricalReadFixture {
    fn new(settings: StorageSettings) -> Self {
        let factory = create_test_provider_factory();
        init_genesis_with_settings(&factory, settings).expect("genesis should initialize");
        factory.set_storage_settings_cache(settings);

        let genesis_hash = factory
            .provider()
            .expect("provider should open")
            .block_hash(0)
            .expect("genesis hash lookup should succeed")
            .expect("genesis hash should exist");
        let mut builder = TestBlockBuilder::eth().with_state();
        let signer = builder.signer;
        let mut parent_hash = genesis_hash;
        let blocks = (1..=HISTORY_BLOCKS)
            .map(|number| {
                let block = builder.get_executed_block_with_number(number, parent_hash);
                parent_hash = block.recovered_block().hash();
                block
            })
            .collect();

        let provider_rw = factory.database_provider_rw().expect("write provider should open");
        provider_rw
            .save_blocks(&SaveBlocksInput::new(blocks, 0, 0, HISTORY_BLOCKS, HISTORY_BLOCKS))
            .expect("history fixture should persist");
        provider_rw.commit().expect("history fixture should commit");

        let provider = factory.provider().expect("sample provider should open");
        let account_query_blocks = (2..=HISTORY_BLOCKS)
            .filter(|block| {
                provider
                    .account_block_changeset(*block)
                    .expect("account changeset read should succeed")
                    .iter()
                    .any(|entry| entry.address == signer)
            })
            .map(|block| block - 1)
            .step_by(8)
            .filter(|block| {
                factory
                    .history_by_block_number(*block)
                    .expect("historical provider should open")
                    .basic_account(&signer)
                    .expect("account read should succeed")
                    .is_some()
            })
            .collect::<Vec<_>>();
        let storage_query_blocks = (2..=HISTORY_BLOCKS)
            .filter(|block| {
                provider
                    .storage_block_changeset(*block)
                    .expect("storage changeset read should succeed")
                    .iter()
                    .any(|entry| entry.address == STORAGE_ADDRESS && entry.key == STORAGE_SLOT)
            })
            .map(|block| block - 1)
            .step_by(8)
            .collect::<Vec<_>>();
        drop(provider);

        assert!(!account_query_blocks.is_empty());
        assert!(!storage_query_blocks.is_empty());

        Self { factory, signer, account_query_blocks, storage_query_blocks }
    }
}

fn historical_read_benches(c: &mut Criterion) {
    let fixtures = [
        ("v1", HistoricalReadFixture::new(StorageSettings::v1())),
        ("v2", HistoricalReadFixture::new(StorageSettings::v2())),
    ];

    let mut group = c.benchmark_group("historical_provider");
    group.sample_size(20);
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(10));
    group.throughput(Throughput::Elements(1));

    for (layout, fixture) in &fixtures {
        let block_index = Cell::new(0);
        group.bench_with_input(BenchmarkId::new("open", layout), fixture, |b, fixture| {
            b.iter(|| {
                let index = block_index.get();
                block_index.set((index + 1) % fixture.storage_query_blocks.len());
                black_box(
                    fixture
                        .factory
                        .history_by_block_number(fixture.storage_query_blocks[index])
                        .expect("historical provider should open"),
                )
            });
        });

        let account_index = Cell::new(0);
        group.bench_with_input(
            BenchmarkId::new("account_before_change", layout),
            fixture,
            |b, fixture| {
                b.iter(|| {
                    let index = account_index.get();
                    account_index.set((index + 1) % fixture.account_query_blocks.len());
                    let state = fixture
                        .factory
                        .history_by_block_number(fixture.account_query_blocks[index])
                        .expect("historical provider should open");
                    black_box(
                        state
                            .basic_account(&fixture.signer)
                            .expect("historical account read should succeed"),
                    )
                });
            },
        );

        let storage_index = Cell::new(0);
        group.bench_with_input(
            BenchmarkId::new("storage_before_change", layout),
            fixture,
            |b, fixture| {
                b.iter(|| {
                    let index = storage_index.get();
                    storage_index.set((index + 1) % fixture.storage_query_blocks.len());
                    let state = fixture
                        .factory
                        .history_by_block_number(fixture.storage_query_blocks[index])
                        .expect("historical provider should open");
                    black_box(
                        state
                            .storage(STORAGE_ADDRESS, STORAGE_SLOT)
                            .expect("historical storage read should succeed"),
                    )
                });
            },
        );

        let state = fixture
            .factory
            .history_by_block_number(HISTORY_BLOCKS / 2)
            .expect("reused historical provider should open");
        group.bench_with_input(
            BenchmarkId::new("account_reused_provider", layout),
            fixture,
            |b, fixture| {
                b.iter(|| {
                    black_box(
                        state
                            .basic_account(&fixture.signer)
                            .expect("historical account read should succeed"),
                    )
                });
            },
        );
        group.bench_with_input(
            BenchmarkId::new("storage_reused_provider", layout),
            fixture,
            |b, _| {
                b.iter(|| {
                    black_box(
                        state
                            .storage(STORAGE_ADDRESS, STORAGE_SLOT)
                            .expect("historical storage read should succeed"),
                    )
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, historical_read_benches);
criterion_main!(benches);

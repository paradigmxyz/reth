//! Benchmarks historical-state reads across Reth storage layouts.

#![allow(missing_docs)]

use std::{cell::Cell, hint::black_box, time::Duration};

use alloy_consensus::Header;
use alloy_primitives::{map::HashMap, Address, B256, U256};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use reth_db_api::models::StorageSettings;
use reth_db_common::init::init_genesis_with_settings;
use reth_ethereum_primitives::Block;
use reth_primitives_traits::RecoveredBlock;
use reth_provider::{
    test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
    AccountReader, BlockHashReader, BlockWriter, DBProvider, DatabaseProviderFactory,
    ExecutionOutcome, ProviderFactory, StorageSettingsCache,
};
use reth_trie::{HashedPostState, KeccakKeyHasher};
use revm::{database::BundleState, state::AccountInfo};

const HISTORY_BLOCKS: u64 = 128;
const ACCOUNT_COUNT: usize = 256;
const SLOTS_PER_ACCOUNT: usize = 4;
const ACCOUNT_QUERY_POSITIONS: [usize; 5] = [0, 63, 127, 191, 255];
const STORAGE_QUERY_POSITIONS: [(usize, usize); 5] =
    [(0, 0), (63, 1), (127, 2), (191, 3), (255, 0)];
const QUERY_BLOCKS: [u64; 6] = [1, 16, 32, 64, 96, 127];

struct HistoricalReadFixture {
    factory: ProviderFactory<MockNodeTypesWithDB>,
    account_queries: Vec<(Address, u64)>,
    storage_queries: Vec<(Address, B256, u64)>,
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
        let blocks = Self::blocks(genesis_hash);
        let bundle = Self::bundle();
        let hashed_state =
            HashedPostState::from_bundle_state::<KeccakKeyHasher>(bundle.state()).into_sorted();
        let execution_outcome =
            ExecutionOutcome::new(bundle, vec![Vec::new(); HISTORY_BLOCKS as usize], 1, Vec::new());

        let provider_rw = factory.database_provider_rw().expect("write provider should open");
        provider_rw
            .append_blocks_with_state(blocks, &execution_outcome, hashed_state)
            .expect("history fixture should persist");
        provider_rw.commit().expect("history fixture should commit");

        let account_queries = QUERY_BLOCKS
            .into_iter()
            .flat_map(|block| {
                ACCOUNT_QUERY_POSITIONS
                    .into_iter()
                    .map(move |position| (Self::address(position), block))
            })
            .collect::<Vec<_>>();
        let storage_queries = QUERY_BLOCKS
            .into_iter()
            .flat_map(|block| {
                STORAGE_QUERY_POSITIONS.into_iter().map(move |(account, slot)| {
                    (Self::address(account), B256::from(Self::slot(slot)), block)
                })
            })
            .collect::<Vec<_>>();

        let provider = factory.provider().expect("sample provider should open");
        for &(address, block) in &account_queries {
            let account = provider
                .history_by_block_number(block)
                .expect("historical provider should open")
                .basic_account(&address)
                .expect("account read should succeed")
                .expect("fixture account should exist");
            let position = address.as_slice()[Address::len_bytes() - 1] as usize;
            assert_eq!(account.nonce, block);
            assert_eq!(account.balance, Self::balance(block, position));
        }
        for &(address, slot, block) in &storage_queries {
            let account = address.as_slice()[Address::len_bytes() - 1] as usize;
            let slot_position = U256::from_be_bytes(slot.0) - U256::from(1);
            let value = provider
                .history_by_block_number(block)
                .expect("historical provider should open")
                .storage(address, slot)
                .expect("storage read should succeed")
                .expect("fixture storage should exist");
            assert_eq!(value, Self::storage_value(block, account, slot_position.to::<usize>()));
        }
        drop(provider);

        Self { factory, account_queries, storage_queries }
    }

    fn blocks(mut parent_hash: B256) -> Vec<RecoveredBlock<Block>> {
        (1..=HISTORY_BLOCKS)
            .map(|number| {
                let block = RecoveredBlock::new_unhashed(
                    Block {
                        header: Header {
                            parent_hash,
                            number,
                            timestamp: number,
                            difficulty: U256::from(1),
                            ..Default::default()
                        },
                        body: Default::default(),
                    },
                    Vec::new(),
                );
                parent_hash = block.hash();
                block
            })
            .collect()
    }

    fn bundle() -> BundleState {
        type Revert = Vec<(Address, Option<Option<AccountInfo>>, Vec<(U256, U256)>)>;

        let state = (0..ACCOUNT_COUNT).map(|account| {
            let storage = (0..SLOTS_PER_ACCOUNT)
                .map(|slot| {
                    (
                        Self::slot(slot),
                        (U256::ZERO, Self::storage_value(HISTORY_BLOCKS, account, slot)),
                    )
                })
                .collect::<HashMap<_, _>>();
            (
                Self::address(account),
                None,
                Some(Self::account_info(HISTORY_BLOCKS, account)),
                storage,
            )
        });
        let reverts = (1..=HISTORY_BLOCKS).map(|block| {
            (0..ACCOUNT_COUNT)
                .map(|account| {
                    let account_revert = if block == 1 {
                        Some(None)
                    } else {
                        Some(Some(Self::account_info(block - 1, account)))
                    };
                    let storage_reverts = (0..SLOTS_PER_ACCOUNT)
                        .map(|slot| {
                            let value = if block == 1 {
                                U256::ZERO
                            } else {
                                Self::storage_value(block - 1, account, slot)
                            };
                            (Self::slot(slot), value)
                        })
                        .collect();
                    (Self::address(account), account_revert, storage_reverts)
                })
                .collect::<Revert>()
        });

        BundleState::new(state, reverts, [])
    }

    const fn address(position: usize) -> Address {
        Address::with_last_byte(position as u8)
    }

    fn account_info(block: u64, position: usize) -> AccountInfo {
        AccountInfo { nonce: block, balance: Self::balance(block, position), ..Default::default() }
    }

    fn balance(block: u64, position: usize) -> U256 {
        U256::from(block * ACCOUNT_COUNT as u64 + position as u64 + 1)
    }

    fn slot(position: usize) -> U256 {
        U256::from(position + 1)
    }

    fn storage_value(block: u64, account: usize, slot: usize) -> U256 {
        U256::from(
            block * (ACCOUNT_COUNT * SLOTS_PER_ACCOUNT) as u64 +
                (account * SLOTS_PER_ACCOUNT + slot + 1) as u64,
        )
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
                block_index.set((index + 1) % fixture.storage_queries.len());
                black_box(
                    fixture
                        .factory
                        .history_by_block_number(fixture.storage_queries[index].2)
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
                    account_index.set((index + 1) % fixture.account_queries.len());
                    let (address, block) = fixture.account_queries[index];
                    let state = fixture
                        .factory
                        .history_by_block_number(block)
                        .expect("historical provider should open");
                    black_box(
                        state
                            .basic_account(&address)
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
                    storage_index.set((index + 1) % fixture.storage_queries.len());
                    let (address, slot, block) = fixture.storage_queries[index];
                    let state = fixture
                        .factory
                        .history_by_block_number(block)
                        .expect("historical provider should open");
                    black_box(
                        state
                            .storage(address, slot)
                            .expect("historical storage read should succeed"),
                    )
                });
            },
        );

        let state = fixture
            .factory
            .history_by_block_number(HISTORY_BLOCKS / 2)
            .expect("reused historical provider should open");
        let reused_account_index = Cell::new(0);
        group.bench_with_input(
            BenchmarkId::new("account_reused_provider", layout),
            fixture,
            |b, _| {
                b.iter(|| {
                    let index = reused_account_index.get();
                    reused_account_index.set((index + 1) % ACCOUNT_QUERY_POSITIONS.len());
                    black_box(
                        state
                            .basic_account(&HistoricalReadFixture::address(
                                ACCOUNT_QUERY_POSITIONS[index],
                            ))
                            .expect("historical account read should succeed"),
                    )
                });
            },
        );
        let reused_storage_index = Cell::new(0);
        group.bench_with_input(
            BenchmarkId::new("storage_reused_provider", layout),
            fixture,
            |b, _| {
                b.iter(|| {
                    let index = reused_storage_index.get();
                    reused_storage_index.set((index + 1) % STORAGE_QUERY_POSITIONS.len());
                    let (account, slot) = STORAGE_QUERY_POSITIONS[index];
                    black_box(
                        state
                            .storage(
                                HistoricalReadFixture::address(account),
                                B256::from(HistoricalReadFixture::slot(slot)),
                            )
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

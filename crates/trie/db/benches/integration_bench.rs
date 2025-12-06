#![allow(missing_docs, unreachable_pub)]

use alloy_primitives::{Address, B256, U256};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use rand::{rngs::StdRng, Rng, SeedableRng};
use reth_db::{
    test_utils::create_test_rw_db,
    transaction::{DbTx, DbTxMut},
    Database, DatabaseEnv,
};
use reth_db_api::{
    models::{AccountBeforeTx, BlockNumberAddress},
    tables,
};
use reth_primitives_traits::{Account, StorageEntry};
use reth_trie::{HashedPostStateSorted, KeccakKeyHasher};
use reth_trie_db::DatabaseHashedPostState;
use std::sync::Arc;

// Populate a temporary database with synthetic revert data.
type TestDb = Arc<reth_db::test_utils::TempDatabase<DatabaseEnv>>;

#[derive(Clone, Copy, Debug)]
enum Distribution {
    Uniform(usize),
    Skewed { light: usize, heavy: usize },
}

fn seed_db(num_accounts: usize, dist: Distribution) -> TestDb {
    let db = create_test_rw_db();
    let tx = db.tx_mut().expect("failed to create rw tx");

    let mut rng = StdRng::seed_from_u64(12345);
    let block = 1;

    for idx in 0..num_accounts {
        let address = Address::random();

        let account =
            Account { nonce: idx as u64, balance: U256::from(idx as u64), bytecode_hash: None };

        tx.put::<tables::AccountChangeSets>(
            block,
            AccountBeforeTx { address, info: Some(account) },
        )
        .expect("failed to insert account changeset");

        let slots_count = match dist {
            Distribution::Uniform(n) => n,
            Distribution::Skewed { light, heavy } => {
                if rng.random_bool(0.05) {
                    heavy
                } else {
                    light
                }
            }
        };

        for slot_idx in 0..slots_count {
            let slot_key = B256::random();
            let value = U256::from(slot_idx);

            tx.put::<tables::StorageChangeSets>(
                BlockNumberAddress((block, address)),
                StorageEntry { key: slot_key, value },
            )
            .expect("failed to insert storage changeset");
        }
    }

    tx.commit().expect("failed to commit seeded db");
    db
}

fn bench_db_execution(c: &mut Criterion) {
    let mut group = c.benchmark_group("Integration_DB_Sort");
    group.measurement_time(std::time::Duration::from_secs(10));

    // Scenarios to test:
    // - Below Threshold: Should be identical (Sequential)
    // - Above Threshold: Should show some speedup with feature flag
    // - Skewed: Should show speedup with feature flag
    let scenarios = vec![
        ("Small_Under_Threshold", 1_000, Distribution::Uniform(50)),
        ("Large_Uniform", 10_000, Distribution::Uniform(20)),
        ("Large_Skewed", 10_000, Distribution::Skewed { light: 4, heavy: 2_000 }),
    ];

    for (name, accounts, dist) in scenarios {
        let db = seed_db(accounts, dist);
        let range = 1..=1;

        group.bench_function(BenchmarkId::new("from_reverts", name), |b| {
            b.iter(|| {
                let tx = db.tx().expect("failed to create ro tx");

                let state =
                    HashedPostStateSorted::from_reverts::<KeccakKeyHasher>(&tx, range.clone())
                        .expect("failed to calculate state");

                black_box(state);
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_db_execution);
criterion_main!(benches);

#![allow(missing_docs, unreachable_pub)]

use alloy_primitives::{Address, B256, U256};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
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
use reth_trie::{HashedPostState, HashedPostStateSorted, KeccakKeyHasher};
use reth_trie_db::{DatabaseHashedPostState, DatabaseHashedPostStateSorted};
use std::sync::Arc;

/// Populate a temporary database with synthetic revert data.
type TestDb = Arc<reth_db::test_utils::TempDatabase<DatabaseEnv>>;

fn seed_db(accounts: usize, slots_per_account: usize) -> TestDb {
    let db = create_test_rw_db();
    let tx = db.tx_mut().expect("failed to create rw tx");

    // Use a single block for all change entries to keep iteration simple.
    let block = 1;

    for idx in 0..accounts {
        // Derive a unique address from the loop index.
        let mut bytes = [0u8; 20];
        let value = (idx + 1) as u16;
        bytes[18] = (value >> 8) as u8;
        bytes[19] = value as u8;
        let address = Address::from(bytes);
        let account = Account {
            nonce: idx as u64,
            balance: U256::from(idx as u64),
            bytecode_hash: None,
        };
        tx.put::<tables::AccountChangeSets>(block, AccountBeforeTx { address, info: Some(account) })
            .expect("failed to insert account changeset");

        for slot_idx in 0..slots_per_account {
            let slot = B256::from(U256::from((slot_idx + 1) as u64));
            let value = U256::from(((idx + 1) * (slot_idx + 1)) as u64);
            tx.put::<tables::StorageChangeSets>(
                BlockNumberAddress((block, address)).into(),
                StorageEntry { key: slot, value },
            )
            .expect("failed to insert storage changeset");
        }
    }

    tx.commit().expect("failed to commit seeded db");
    db
}

pub fn from_reverts_sorted_vs_unsorted(c: &mut Criterion) {
    // Keep dataset moderate so the benchmark remains quick while exercising both paths.
    let accounts = 256;
    let slots_per_account = 4;
    let range = 1..=1;
    let db = seed_db(accounts, slots_per_account);

    let mut group = c.benchmark_group("from_reverts_sorted_vs_unsorted");

    group.bench_function("unsorted_then_sort", |b| {
        b.iter(|| {
            let tx = db.tx().expect("failed to create ro tx");
            let state = HashedPostState::from_reverts::<KeccakKeyHasher>(&tx, range.clone())
                .expect("unsorted reverts")
                .into_sorted();
            black_box(state);
        });
    });

    group.bench_function("direct_sorted", |b| {
        b.iter(|| {
            let tx = db.tx().expect("failed to create ro tx");
            let state = HashedPostStateSorted::from_reverts::<KeccakKeyHasher>(&tx, range.clone())
                .expect("sorted reverts");
            black_box(state);
        });
    });

    group.finish();
}

criterion_group!(from_reverts, from_reverts_sorted_vs_unsorted);
criterion_main!(from_reverts);

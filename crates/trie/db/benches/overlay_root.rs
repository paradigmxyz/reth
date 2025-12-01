#![allow(missing_docs)]

use alloy_primitives::{Address, B256, U256};
use criterion::{criterion_group, criterion_main, BatchSize, Criterion, SamplingMode, Throughput};
use reth_db::{test_utils::create_test_rw_db, transaction::DbTx};
use reth_db_api::{models::BlockNumberAddress, tables, transaction::DbTxMut, Database};
use reth_primitives_traits::Account;
use reth_trie::{HashedPostState, HashedPostStateSorted, KeccakKeyHasher, StateRoot};
use reth_trie_db::{DatabaseHashedPostState, DatabaseHashedPostStateSorted, DatabaseStateRoot};

/// Seed a temporary DB with account/storage changesets for a fixed block.
fn seed_changesets<T: DbTxMut>(tx: &T) {
    const ACCOUNTS: usize = 256;
    const SLOTS_PER_ACCOUNT: usize = 4;
    const BLOCK: u64 = 1;

    for i in 0..ACCOUNTS {
        let addr_bytes = U256::from(i as u64).to_be_bytes::<32>();
        let address = Address::from_slice(&addr_bytes[12..]);
        let account = Account { nonce: i as u64, balance: U256::from(i as u64), bytecode_hash: None };
        tx.put::<tables::AccountChangeSets>(
            BLOCK,
            reth_db_api::models::AccountBeforeTx { address, info: Some(account) },
        )
        .unwrap();

        for j in 0..SLOTS_PER_ACCOUNT {
            let slot = B256::from(U256::from((i * SLOTS_PER_ACCOUNT + j) as u64));
            let value = U256::from((i + j) as u64);
            tx.put::<tables::StorageChangeSets>(
                BlockNumberAddress((BLOCK, address)).into(),
                reth_primitives_traits::StorageEntry { key: slot, value },
            )
            .unwrap();
        }
    }
}

fn bench_overlay_root(c: &mut Criterion) {
    let db = create_test_rw_db();
    {
        let tx = db.tx_mut().unwrap();
        seed_changesets(&tx);
        tx.commit().unwrap();
    }

    let tx = db.tx().unwrap();
    let unsorted = HashedPostState::from_reverts::<KeccakKeyHasher>(&tx, 1..=1).unwrap();
    let sorted = HashedPostStateSorted::from_reverts::<KeccakKeyHasher>(&tx, 1..=1).unwrap();

    let mut group = c.benchmark_group("overlay_root");
    group.sampling_mode(SamplingMode::Flat);
    group.throughput(Throughput::Elements(1));

    group.bench_function("unsorted", |b| {
        b.iter_batched(
            || unsorted.clone(),
            |state| StateRoot::overlay_root(&tx, state).unwrap(),
            BatchSize::SmallInput,
        )
    });

    group.bench_function("sorted", |b| {
        b.iter_batched_ref(
            || (),
            |_| StateRoot::overlay_root_sorted(&tx, &sorted).unwrap(),
            BatchSize::SmallInput,
        )
    });

    group.finish();
}

criterion_group!(name = benches; config = Criterion::default(); targets = bench_overlay_root);
criterion_main!(benches);

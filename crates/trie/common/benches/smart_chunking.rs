#![allow(missing_docs, unreachable_pub)]

use alloy_primitives::{keccak256, map::HashSet, Address, B256, U256};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rand::{rngs::StdRng, Rng, SeedableRng};
use reth_db::{
    cursor::DbDupCursorRO,
    tables,
    test_utils::create_test_rw_db,
    transaction::{DbTx, DbTxMut},
    Database, DatabaseEnv,
};
use reth_primitives::StorageEntry;
use reth_trie_common::proofs::{
    ChunkedMultiProofTargets, MultiProofTargets, SmartChunkedMultiProofTargets,
};
use std::sync::Arc;

// Constant chunk size
const CHUNK_SIZE: usize = 60;

// fn seed_fragmentation_db(num_accounts: usize, slots_per_account: usize) ->
// (Arc<reth_db::test_utils::TempDatabase<DatabaseEnv>>, MultiProofTargets) {     let db =
// create_test_rw_db();     let tx = db.tx_mut().expect("failed to create rw tx");
//     let mut targets = MultiProofTargets::default();

//     for i in 0..num_accounts {
//         let address = Address::from_word(B256::from(U256::from(i)));
//         let hashed_address = keccak256(address);

//         let mut slot_keys = HashSet::default();

//         for j in 0..slots_per_account {
//             let slot_key = B256::from(U256::from(j));
//             let hashed_slot = keccak256(slot_key);
//             let value = U256::from(1);

//             tx.put::<tables::HashedStorages>(
//                 hashed_address,
//                 StorageEntry { key: hashed_slot, value }
//             ).expect("failed to insert");

//             slot_keys.insert(hashed_slot);
//         }

//         targets.0.insert(hashed_address, slot_keys);
//     }

//     tx.commit().expect("failed to commit");
//     (db, targets)
// }

fn seed_realistic_db(
    num_accounts: usize,
) -> (Arc<reth_db::test_utils::TempDatabase<DatabaseEnv>>, MultiProofTargets) {
    let db = create_test_rw_db();
    let tx = db.tx_mut().expect("failed to create rw tx");
    let mut targets = MultiProofTargets::default();

    let mut rng = StdRng::seed_from_u64(42);

    for i in 0..num_accounts {
        let address = Address::from_word(B256::from(U256::from(i)));
        let hashed_address = keccak256(address);

        // Distribution
        // 80% 1-5 slots
        // 15% 10-50 slots
        // 5% 100-300 slots
        let roll = rng.random_range(0..100);
        let slots_count = if roll < 80 {
            rng.random_range(1..=5)
        } else if roll < 95 {
            rng.random_range(10..=50)
        } else {
            rng.random_range(100..=300)
        };

        let mut slot_keys = HashSet::default();

        for j in 0..slots_count {
            let slot_key = B256::from(U256::from(j));
            let hashed_slot = keccak256(slot_key);
            let value = U256::from(1);

            tx.put::<tables::HashedStorages>(
                hashed_address,
                StorageEntry { key: hashed_slot, value },
            )
            .expect("failed to insert");

            slot_keys.insert(hashed_slot);
        }

        targets.0.insert(hashed_address, slot_keys);
    }

    tx.commit().expect("failed to commit");
    (db, targets)
}

fn execute_proof_fetch(
    db: &Arc<reth_db::test_utils::TempDatabase<DatabaseEnv>>,
    chunks: Vec<MultiProofTargets>,
) {
    let tx = db.tx().expect("ro tx");

    for chunk in chunks {
        let mut cursor = tx.cursor_read::<tables::HashedStorages>().expect("cursor");

        for (hashed_addr, slots) in chunk.0 {
            // Seeking the Account
            if let Some(_entry) = cursor.seek_by_key_subkey(hashed_addr, B256::ZERO).expect("seek")
            {
                for slot in slots {
                    black_box(slot);
                }
            }
        }
    }
}

fn bench_chunking_strategies(c: &mut Criterion) {
    let mut group = c.benchmark_group("Smart_Chunking_Experiment");

    let num_accounts = 1000;

    let (db, targets) = seed_realistic_db(num_accounts);

    group.bench_function("old_chunking_logic", |b| {
        b.iter(|| {
            let t = targets.clone();
            let chunker = ChunkedMultiProofTargets::new(t, CHUNK_SIZE);
            let chunks: Vec<_> = chunker.collect();
            execute_proof_fetch(&db, chunks);
        })
    });

    group.bench_function("smart_chunking_logic", |b| {
        b.iter(|| {
            let t = targets.clone();
            let chunker = SmartChunkedMultiProofTargets::new(t, CHUNK_SIZE);
            let chunks: Vec<_> = chunker.collect();
            execute_proof_fetch(&db, chunks);
        })
    });

    group.finish();
}

criterion_group!(benches, bench_chunking_strategies);
criterion_main!(benches);

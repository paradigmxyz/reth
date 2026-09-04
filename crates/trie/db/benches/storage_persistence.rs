//! Compare the original delete/insert writer with the current writer, including durable commit.
//! Run with `cargo bench -p reth-trie-db --bench storage_persistence`.

#[path = "../tests/persistence_support/mod.rs"]
mod persistence_support;

use alloy_primitives::B256;
use persistence_support::{baseline_write, node, open_database, path, snapshot};
use reth_db_api::{
    transaction::{DbTx, DbTxMut},
    Database,
};
use reth_trie::updates::StorageTrieUpdatesSorted;
use reth_trie_db::{
    DatabaseStorageTrieCursor, LegacyKeyAdapter, PackedKeyAdapter, TrieTableAdapter,
};
use std::time::Instant;

fn run<A: TrieTableAdapter>(encoding: &str, case: &str, samples: usize) {
    let address = B256::repeat_byte(1);
    let initial = StorageTrieUpdatesSorted {
        storage_nodes: (0..4096).map(|i| (path(i), Some(node(i as u64, 8)))).collect(),
    };
    let sparse = case.starts_with("sparse_");
    let count = if sparse { 256 } else { 4096 };
    let updates = StorageTrieUpdatesSorted {
        storage_nodes: (0..count)
            .map(|i| {
                let index = if sparse {
                    i * 16
                } else if case == "append" {
                    i + 4096
                } else {
                    i
                };
                let value = match case {
                    "unchanged" => Some(node(index as u64, 8)),
                    "replace" | "sparse_replace" | "append" => Some(node(index as u64 + 4096, 8)),
                    "resize" | "sparse_resize" => {
                        Some(node(index as u64 + 4096, if i % 2 == 0 { 2 } else { 16 }))
                    }
                    "mixed" if i % 4 == 0 => None,
                    "mixed" if i % 4 == 1 => Some(node(i as u64, 8)),
                    "mixed" => Some(node(i as u64 + 4096, 8)),
                    _ => unreachable!(),
                };
                (path(index), value)
            })
            .collect(),
    };
    let mut times = [Vec::new(), Vec::new()];
    for sample in 0..samples + 3 {
        let mut expected = None;
        // Alternate execution order to reduce thermal and scheduling bias.
        for index in 0..2 {
            let optimized = (index + sample) % 2;
            let dir = tempfile::tempdir().unwrap();
            let db = open_database(dir.path());
            let tx = db.tx_mut().unwrap();
            baseline_write::<_, A>(
                &mut tx.cursor_dup_write::<A::StorageTrieTable>().unwrap(),
                address,
                &initial,
            )
            .unwrap();
            tx.commit().unwrap();

            let start = Instant::now();
            let tx = db.tx_mut().unwrap();
            let count = if optimized == 1 {
                DatabaseStorageTrieCursor::<_, A>::new(
                    tx.cursor_dup_write::<A::StorageTrieTable>().unwrap(),
                    address,
                )
                .write_storage_trie_updates_sorted(&updates)
                .unwrap()
            } else {
                baseline_write::<_, A>(
                    &mut tx.cursor_dup_write::<A::StorageTrieTable>().unwrap(),
                    address,
                    &updates,
                )
                .unwrap()
            };
            tx.commit().unwrap();
            let elapsed = start.elapsed().as_nanos();
            assert_eq!(count, updates.storage_nodes.len());
            drop(db);
            let reopened = open_database(dir.path());
            let actual = snapshot::<A>(&reopened.tx().unwrap());
            if let Some(expected) = &expected {
                assert_eq!(&actual, expected);
            } else {
                expected = Some(actual);
            }
            if sample >= 3 {
                times[optimized].push(elapsed);
            }
        }
    }
    for (mode, times) in ["baseline", "optimized"].into_iter().zip(&mut times) {
        times.sort_unstable();
        println!(
            "{encoding}/{case},{mode},{samples},{},{},{}",
            times[samples / 2],
            times[samples / 4],
            times[samples * 3 / 4]
        );
    }
}

fn main() {
    let samples =
        std::env::var("MPT_BENCH_SAMPLES").map(|s| s.parse::<usize>().unwrap()).unwrap_or(30);
    assert!(samples > 0);
    println!("case,mode,samples,median_ns,p25_ns,p75_ns");
    for case in
        ["unchanged", "replace", "resize", "mixed", "sparse_replace", "sparse_resize", "append"]
    {
        run::<LegacyKeyAdapter>("legacy", case, samples);
        run::<PackedKeyAdapter>("packed", case, samples);
    }
}

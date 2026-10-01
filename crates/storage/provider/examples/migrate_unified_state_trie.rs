//! Copy existing RocksDB state trie families into the unified table, then verify every record.
//! Run with `--features state-trie-rocksdb`; the node must be stopped.

use alloy_primitives::B256;
use reth_db::{mdbx::DatabaseArguments, Database};
use reth_db_api::{tables, transaction::DbTx};
use reth_provider::providers::RocksDBProvider;
use reth_trie::{
    proof_v3::ProofCalculator, state_trie_cursor::StateTrieCursorFactory, StateTrieBuilder,
    StateTrieNode,
};
use std::{convert::Infallible, error::Error, time::Instant};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !(args.len() == 2 || (args.len() == 3 && args[2] == "--check")) {
        return Err("usage: migrate_unified_state_trie DATADIR/db DATADIR/rocksdb [--check]".into())
    }
    // Check the frontier before creating a column family or writing anything.
    let db = reth_db::open_db_read_only(&args[0], DatabaseArguments::default())?;
    let tx = db.tx()?;
    let finish = tx.get::<tables::StageCheckpoints>("Finish".into())?.ok_or("missing Finish")?;
    let partial = finish
        .finish_stage_checkpoint()
        .and_then(|c| c.partial_state_trie())
        .unwrap_or(finish.block_number);
    assert_eq!(partial, finish.block_number, "partial_state_trie must equal Finish");
    println!("finish={} partial_state_trie={partial}", finish.block_number);
    let check = args.len() == 3;
    let rocks =
        RocksDBProvider::builder(&args[1]).with_default_tables().with_read_only(check).build()?;
    let mut builder = StateTrieBuilder::default();
    let mut discard = |_, _| Ok::<_, Infallible>(());
    for entry in rocks.iter::<tables::StateTrieAccounts>()? {
        let (path, node) = entry?;
        if let StateTrieNode::Leaf { value, .. } = node {
            builder.push(B256::from_slice(&path.0.pack()), value, &mut discard)?;
        }
    }
    let expected = builder.finish(&mut discard)?;
    println!("expected_state_root={expected}");
    let started = Instant::now();
    if !check {
        println!("migrated_nodes={}", rocks.migrate_unified_state_trie()?);
    }
    println!("verified_nodes={}", rocks.verify_unified_state_trie_migration()?);
    let snapshot = rocks.snapshot();
    let mut proof = ProofCalculator::new(snapshot.state_trie_account_cursor()?);
    let root = proof.root_node()?;
    assert_eq!(proof.compute_root_hash(&[root])?, Some(expected), "migrated state root mismatch");
    println!("verified_state_root={expected} elapsed_secs={:.1}", started.elapsed().as_secs_f64());
    for stat in rocks.table_stats().into_iter().filter(|stat| stat.name.starts_with("StateTrie")) {
        println!(
            "table={} entries={} sst_bytes={}",
            stat.name, stat.estimated_num_keys, stat.sst_size_bytes
        );
    }
    Ok(())
}

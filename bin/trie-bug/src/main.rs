#![allow(unused, missing_docs)]

use alloy_primitives::{hex::FromHex, keccak256, B256};
use reth_chainspec::HOODI;
use reth_db::{cursor::DbCursorRO, open_db, tables, transaction::DbTx};
use reth_node_ethereum::EthereumNode;
use reth_provider::{providers::StaticFileProvider, DatabaseProviderFactory};
use reth_tracing::Tracer;
use reth_trie::{
    hashed_cursor::{HashedCursor, HashedCursorFactory, HashedStorageCursor},
    metrics::TrieRootMetrics,
    node_iter::{TrieElement, TrieNodeIter},
    trie_cursor::TrieCursorFactory,
    walker::TrieWalker,
    KeccakKeyHasher, StateRoot, StorageRoot, TrieType,
};
use reth_trie_common::prefix_set::PrefixSet;
use reth_trie_db::{
    DatabaseHashedCursorFactory, DatabaseStateRoot, DatabaseTrieCursorFactory, PrefixSetLoader,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

fn main() -> eyre::Result<()> {
    let db_path = PathBuf::from(std::env::var("RETH_DB_PATH")?);
    let db = open_db(Path::new(&db_path.join("db")), Default::default())?;
    let provider_factory = EthereumNode::provider_factory_builder()
        .db(Arc::new(db))
        .chainspec(HOODI.clone())
        .static_file(StaticFileProvider::read_only(&db_path.join("static_files"), false).unwrap())
        .build_provider_factory();

    let tx = provider_factory.database_provider_ro()?.into_tx();
    let range = 508..=508;

    /*
    let loaded_prefix_sets = PrefixSetLoader::<_, KeccakKeyHasher>::new(&tx).load(range)?;
    for key in loaded_prefix_sets.account_prefix_set.iter() {
        println!("prefix set key: {key:?}");
    }
    return Ok(());
    */

    let tracer = reth_tracing::TestTracer::default();
    tracer.init().unwrap();

    /*
    let (block_root, updates) = StateRoot::incremental_root_with_updates(&tx, range).unwrap();

    println!("new state root: {block_root}");
    for (acc, node) in updates.account_nodes.iter() {
        println!("acc:{acc:?} -> {node:?}");
    }
    */

    /*
    let hashed_cursor_factory = DatabaseHashedCursorFactory::new(&tx);

    let mut hashed_account_cursor = hashed_cursor_factory.hashed_account_cursor()?;
    let x = hashed_account_cursor.seek(B256::from_hex(
        "0x8ef3bb7bf0fd1cc0e9cb148623ed1d87b12434a1a78370a1067b86baf1c5d84d",
    )?)?;
    println!("{x:?}");
    */

    /*
    let trie_cursor_factory = DatabaseTrieCursorFactory::new(&tx);
    let hashed_cursor_factory = DatabaseHashedCursorFactory::new(&tx);

    let mut hashed_storage_cursor = hashed_cursor_factory.hashed_storage_cursor(hashed_address)?;

    let walker = TrieWalker::storage_trie(
        trie_cursor_factory.storage_trie_cursor(hashed_address)?,
        PrefixSet::default(),
    )
    .with_deletions_retained(true);

    let mut storage_node_iter = TrieNodeIter::storage_trie(walker, hashed_storage_cursor);

    let breaker = nybbles::Nibbles::from_nibbles(&[0x01, 0x06]);
    while let Some(node) = storage_node_iter.try_next()? {
        println!("node:{node:?}");

        if let TrieElement::Branch(node) = node {
            if node.key >= breaker {
                break
            }
        }
    }
    */

    Ok(())
}

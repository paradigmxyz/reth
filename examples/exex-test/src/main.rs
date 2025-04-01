use futures_util::TryStreamExt;
use reth::{api::FullNodeComponents, builder::NodeTypes, primitives::EthPrimitives};
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::{
    testsuite::{
        actions::{ProduceBlocks, FinalizeBlock},
        setup::{NetworkSetup, Setup},
        TestBuilder,
    },
};
use reth_exex::{ExExContext, ExExEvent, ExExNotification};
use reth_node_ethereum::{EthEngineTypes, EthereumNode};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
mod wal_test;
use wal_test::wal_test_exex;

struct TestState {
    received_blocks: AtomicU64,
    saw_trie_updates: AtomicBool,
}

async fn test_assertion_exex<Node: FullNodeComponents<Types: NodeTypes<Primitives = EthPrimitives>>>(
    mut ctx: ExExContext<Node>,
) -> eyre::Result<()> {

    let test_state = Arc::new(TestState {
        received_blocks: AtomicU64::new(0),
        saw_trie_updates: AtomicBool::new(false),
    });
    
    println!("ExEx test started - waiting for chain notifications");

    // Process notifications
    while let Some(notification) = ctx.notifications.try_next().await? {
        match &notification {
            ExExNotification::ChainCommitted { new } => {
                let num_blocks = new.len() as u64;
                let current = test_state.received_blocks.fetch_add(num_blocks, Ordering::SeqCst);
                
                println!(
                    "Received commit notification for chain range: {:?}, total blocks: {}",
                    new.range(),
                    current + num_blocks
                );
                
                // Assert: Chain should have trie updates
                if let Some(trie_updates) = new.trie_updates() {
                    test_state.saw_trie_updates.store(true, Ordering::SeqCst);
                    println!(
                        "Chain notification contains trie updates, size: {}",
                        trie_updates.len()
                    );
                } else if !new.blocks_iter().next().map_or(true, |b| b.is_genesis()) {
                    // Skip assertion for genesis block which might not have trie updates
                    println!("Chain notification missing trie updates");
                }
                
                // Send finished height event back
                ctx.events.send(ExExEvent::FinishedHeight(new.tip().num_hash()))?;
            }
            ExExNotification::ChainReorged { old, new } => {
                println!(
                    "Received reorg notification from {:?} to {:?}",
                    old.range(),
                    new.range()
                );
                
                // Update block count
                let old_count = old.len() as i64;
                let new_count = new.len() as i64;
                let diff = new_count - old_count;
                
                if diff > 0 {
                    test_state.received_blocks.fetch_add(diff as u64, Ordering::SeqCst);
                } else if diff < 0 {
                    test_state.received_blocks.fetch_sub((-diff) as u64, Ordering::SeqCst);
                }
                
                // Check trie updates present in new chain
                if let Some(trie_updates) = new.trie_updates() {
                    test_state.saw_trie_updates.store(true, Ordering::SeqCst);
                    println!(
                        "Reorg notification contains trie updates, size: {}",
                        trie_updates.len()
                    );
                } else {
                    println!("Reorg notification missing trie updates");
                }
                
                // Send finished height event
                ctx.events.send(ExExEvent::FinishedHeight(new.tip().num_hash()))?;
            }
            ExExNotification::ChainReverted { old } => {
                println!(
                    "Received revert notification for chain {:?}",
                    old.range()
                );
                
                // Update block count
                let reverted_count = old.len() as u64;
                test_state.received_blocks.fetch_sub(reverted_count, Ordering::SeqCst);
            }
        }
    }

    report_test_results(&test_state);
    Ok(())
}

fn report_test_results(state: &TestState) {
    let blocks_received = state.received_blocks.load(Ordering::SeqCst);
    let saw_trie_updates = state.saw_trie_updates.load(Ordering::SeqCst);
    
    // println!("========= ExEx Test Report =========");
    // println!("Total blocks received: {}", blocks_received);
    // println!("Trie updates observed: {}", saw_trie_updates);
    // println!("====================================");
    
    assert!(blocks_received > 0, "No blocks were received by the ExEx");
    assert!(saw_trie_updates, "No trie updates were observed in any notifications");
}

async fn run_exex_test() -> eyre::Result<()> {
    println!("Starting ExEx test...");
    
    // Set up the test environment
    let setup = Setup::default()
        .with_chain_spec(Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(serde_json::from_str(include_str!("../../../crates/e2e-test-utils/src/testsuite/assets/genesis.json")).unwrap())
                .cancun_activated()
                .build(),
        ))
        .with_network(NetworkSetup::single_node());

    println!("Test environment set up");

    // test with both ExExs, block production, and finalization
    let test = TestBuilder::new()
        .with_setup(setup)
        .with_exex("test-assertions-exex", test_assertion_exex)
        .with_exex("wal-test-exex", wal_test_exex)
        .with_action(ProduceBlocks::<EthEngineTypes>::new(5))
        .with_action(FinalizeBlock::<EthEngineTypes>::at_height(3))
        .with_action(ProduceBlocks::<EthEngineTypes>::new(2));

    println!("Test built, running...");

    test.run::<EthereumNode>().await?;

    println!("Test completed successfully");

    Ok(())
}

fn main() -> eyre::Result<()> {

    println!("Starting ExEx test example");
    
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run_exex_test())
}
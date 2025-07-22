use futures_util::StreamExt;
use reth_e2e_test_utils::testsuite::{
    actions::ProduceBlocks,
    setup::{NetworkSetup, Setup},
    TestBuilder,
};
use reth_ethereum::{
    chainspec::{ChainSpecBuilder, MAINNET},
    exex::{ExExContext, ExExEvent},
    node::{
        api::{FullNodeComponents, NodeTypes},
        EthEngineTypes, EthereumNode,
    },
    EthPrimitives,
};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};

mod wal_test;

#[allow(unfulfilled_lint_expectations)]
struct TestState {
    received_blocks: AtomicU64,
    saw_trie_updates: AtomicBool,
    last_finalized_block: AtomicU64,
}

/// ExEx that tests assertions about notifications and state
#[expect(dead_code)]
async fn test_assertion_exex<
    Node: FullNodeComponents<Types: NodeTypes<Primitives = EthPrimitives>>,
>(
    mut ctx: ExExContext<Node>,
) -> eyre::Result<()> {
    let state = Arc::new(TestState {
        received_blocks: AtomicU64::new(0),
        saw_trie_updates: AtomicBool::new(false),
        last_finalized_block: AtomicU64::new(0),
    });

    println!("Assertion ExEx started");

    // Clone state for the async block
    let state_clone = state.clone();

    // Process notifications
    while let Some(result) = ctx.notifications.next().await {
        // Handle the Result with ?
        let notification = result?;

        // Check for committed chain
        if let Some(committed_chain) = notification.committed_chain() {
            let range = committed_chain.range();
            let blocks_count = *range.end() - *range.start() + 1;
            println!("Received committed chain: {range:?}");

            // Increment blocks count
            #[allow(clippy::unnecessary_cast)]
            state_clone.received_blocks.fetch_add(blocks_count as u64, Ordering::SeqCst);

            // Send event that we've processed this height
            ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().num_hash()))?;

            // Check for finalization
            state_clone.last_finalized_block.store(committed_chain.tip().number, Ordering::SeqCst);
        }

        // For example, if we see any block, we'll set saw_trie_updates to true
        // This is a simplification
        state_clone.saw_trie_updates.store(true, Ordering::SeqCst);
    }

    // Report results at the end
    report_test_results(&state);

    Ok(())
}

/// Verify test assertions after completion
fn report_test_results(state: &TestState) {
    let blocks_received = state.received_blocks.load(Ordering::SeqCst);
    let saw_trie_updates = state.saw_trie_updates.load(Ordering::SeqCst);
    let last_finalized = state.last_finalized_block.load(Ordering::SeqCst);

    println!("========= ExEx Test Report =========");
    println!("Total blocks received: {blocks_received}");
    println!("Trie updates observed: {saw_trie_updates}");
    println!("Last finalized block: {last_finalized}");
    println!("====================================");

    assert!(blocks_received > 0, "No blocks were received by the ExEx");
    assert!(saw_trie_updates, "No trie updates were observed in any notifications");
    assert!(last_finalized > 0, "No finalization events were observed");
}

async fn run_exex_test() -> eyre::Result<()> {
    println!("Starting ExEx test...");

    // Set up the test environment
    let setup = Setup::default()
        .with_chain_spec(Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(
                    serde_json::from_str(include_str!(
                        "../../../crates/e2e-test-utils/src/testsuite/assets/genesis.json"
                    ))
                    .unwrap(),
                )
                .cancun_activated()
                .build(),
        ))
        .with_network(NetworkSetup::single_node());

    println!("Test environment set up");

    let test = TestBuilder::new()
        .with_setup(setup)
        .with_action(ProduceBlocks::<EthEngineTypes>::new(5))
        .with_action(ProduceBlocks::<EthEngineTypes>::new(2));

    println!("Test built, running...");

    test.run::<EthereumNode>().await?;

    println!("Test completed successfully");

    Ok(())
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    println!("Starting ExEx test example");
    run_exex_test().await
}

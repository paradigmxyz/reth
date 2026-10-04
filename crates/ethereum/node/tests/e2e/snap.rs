//! End-to-end sync scenarios with `--snap.v2`.
//!
//! - A fresh node snap-syncs 100 finalized Amsterdam blocks from a serving peer, with a matching
//!   head state root and a verified snap attempt.
//! - A fresh node syncs 100 finalized Prague blocks through the staged pipeline without starting a
//!   snap attempt.

use crate::utils::advance_with_random_transactions;
use rand::{rngs::StdRng, SeedableRng};
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::{E2ETestSetupBuilder, E2ETestSetupExt, NodeHelperType};
use reth_node_ethereum::{snap::EthereumBackfill, EthereumNode};
use reth_provider::{DatabaseProviderFactory, HeaderProvider, MetadataProvider};
use reth_tasks::Runtime;

// Blocks the serving node builds: more than the backfill threshold and the snap pivot distance,
// and within the blocks peers serve state for.
const CHAIN_LENGTH: u64 = 100;

// A node on `fork` that serves snap/2 from the storage layout snap writes.
fn snap_setup(fork: EthereumHardfork, runtime: Runtime) -> E2ETestSetupBuilder<EthereumNode> {
    EthereumNode::test_setup_for(fork)
        .with_runtime(runtime)
        .with_storage_v2(true)
        .with_node_config_modifier(|mut config| {
            config.network.snap_v2 = true;
            config
        })
}

// A serving node with `CHAIN_LENGTH` finalized blocks of random transactions, and a fresh node
// that syncs with the snap backfill, connected to it.
async fn serving_and_syncing(
    fork: EthereumHardfork,
) -> eyre::Result<(NodeHelperType<EthereumNode>, NodeHelperType<EthereumNode>)> {
    let runtime = Runtime::test();
    let (mut server, _) = snap_setup(fork, runtime.clone()).build_single().await?;
    let mut rng = StdRng::seed_from_u64(1);
    advance_with_random_transactions(&mut server, CHAIN_LENGTH as usize, &mut rng, true).await?;

    let (mut client, _) =
        snap_setup(fork, runtime).with_backfill(EthereumBackfill::Snap).build_single().await?;
    client.connect(&mut server).await;
    Ok((server, client))
}

#[tokio::test]
async fn a_fresh_node_snap_syncs_to_the_head() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let (server, client) = serving_and_syncing(EthereumHardfork::Amsterdam).await?;

    client.sync_to(server.block_hash(CHAIN_LENGTH)).await?;

    let state_root = |node: &NodeHelperType<EthereumNode>| {
        node.inner.provider.sealed_header(CHAIN_LENGTH).unwrap().unwrap().state_root
    };
    assert_eq!(state_root(&client), state_root(&server));
    // The state came from snap, not from executing the chain.
    let attempt = client.inner.provider.database_provider_ro()?.snap_attempt()?;
    assert!(attempt.is_some_and(|attempt| attempt.is_verified()));
    Ok(())
}

#[tokio::test]
async fn a_chain_before_amsterdam_syncs_with_the_staged_pipeline() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let (server, client) = serving_and_syncing(EthereumHardfork::Prague).await?;

    client.sync_to(server.block_hash(CHAIN_LENGTH)).await?;

    assert!(client.inner.provider.database_provider_ro()?.snap_attempt()?.is_none());
    Ok(())
}

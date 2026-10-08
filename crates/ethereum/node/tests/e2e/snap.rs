//! End-to-end sync scenarios with `--snap.v2`.
//!
//! - A fresh node snap-syncs 100 finalized Amsterdam blocks from a serving peer, with persisted
//!   state and trie nodes matching the head state root and a verified snap attempt.
//! - A fresh node syncs 100 finalized Prague blocks through the staged pipeline without starting a
//!   snap attempt.
//! - A snap-synced node reads downloaded contract code and storage, then executes a new call that
//!   updates that storage.

use crate::utils::advance_with_random_transactions;
use alloy_primitives::{bytes, Bytes, U256};
use alloy_provider::Provider;
use rand::{rngs::StdRng, SeedableRng};
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::{
    trie::assert_trie_consistency, E2ETestSetupBuilder, E2ETestSetupExt, NodeHelperType,
};
use reth_node_ethereum::{snap::EthereumBackfill, EthereumNode};
use reth_provider::{
    DatabaseProviderFactory, HeaderProvider, MetadataProvider, StageCheckpointReader,
};
use reth_stages_types::StageId;
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
        .with_backfill(EthereumBackfill::new)
}

// A serving node with `CHAIN_LENGTH` finalized blocks of random transactions, and a fresh node
// connected to it.
async fn serving_and_syncing(
    fork: EthereumHardfork,
) -> eyre::Result<(NodeHelperType<EthereumNode>, NodeHelperType<EthereumNode>)> {
    let runtime = Runtime::test();
    let (mut server, _) = snap_setup(fork, runtime.clone()).build_single().await?;
    let mut rng = StdRng::seed_from_u64(1);
    advance_with_random_transactions(&mut server, CHAIN_LENGTH as usize, &mut rng, true).await?;

    let client = syncing_client(&mut server, snap_setup(fork, runtime)).await?;
    Ok((server, client))
}

// A fresh node launched from `setup` and connected to `server`.
async fn syncing_client(
    server: &mut NodeHelperType<EthereumNode>,
    setup: E2ETestSetupBuilder<EthereumNode>,
) -> eyre::Result<NodeHelperType<EthereumNode>> {
    let (mut client, _) = setup.build_single().await?;
    client.connect(server).await;
    Ok(client)
}

#[tokio::test]
async fn a_fresh_node_snap_syncs_to_the_head() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let (server, client) = serving_and_syncing(EthereumHardfork::Amsterdam).await?;

    client.sync_to(server.block_hash(CHAIN_LENGTH)).await?;

    client.wait_for_persisted_block(CHAIN_LENGTH).await?;
    assert_trie_consistency(&client.inner.provider)?;
    // The state came from snap, not from executing the chain.
    let attempt = client.inner.provider.database_provider_ro()?.snap_attempt()?.unwrap();
    let head = server.inner.provider.sealed_header(CHAIN_LENGTH)?.unwrap();
    assert!(attempt.is_verified());
    assert_eq!(attempt.pivot(), head.num_hash());
    assert_eq!(attempt.state_root(), head.state_root);
    Ok(())
}

#[tokio::test]
async fn a_chain_before_amsterdam_syncs_with_the_staged_pipeline() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let (server, client) = serving_and_syncing(EthereumHardfork::Prague).await?;

    client.sync_to(server.block_hash(CHAIN_LENGTH)).await?;

    client.wait_for_persisted_block(CHAIN_LENGTH).await?;
    assert_trie_consistency(&client.inner.provider)?;
    assert!(client.inner.provider.database_provider_ro()?.snap_attempt()?.is_none());
    // The pipeline executed the chain.
    let execution = client.inner.provider.get_stage_checkpoint(StageId::Execution)?.unwrap();
    assert_eq!(execution.block_number, CHAIN_LENGTH);
    Ok(())
}

#[tokio::test]
async fn a_snap_synced_node_executes_downloaded_contract_code() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let runtime = Runtime::test();
    let (mut server, wallet) =
        snap_setup(EthereumHardfork::Amsterdam, runtime.clone()).build_single().await?;
    let mut account = wallet.account(0);

    // The runtime increments slot 0: PUSH0 SLOAD PUSH1 1 ADD PUSH0 SSTORE STOP.
    let code = bytes!("5f546001015f5500");
    let init_code = bytes!("675f546001015f55005f5260086018f3");
    let contract = account.next_contract_address();
    server.mine([account.deploy(init_code).await]).await?.ensure_success()?;
    server.mine([account.call(contract, Bytes::new()).await]).await?.ensure_success()?;
    server.advance_blocks(CHAIN_LENGTH - 2).await?;

    let setup =
        snap_setup(EthereumHardfork::Amsterdam, runtime).with_tree_config_modifier(|config| {
            config.with_persistence_threshold(0).with_memory_block_buffer_target(0)
        });
    let client = syncing_client(&mut server, setup).await?;
    client.sync_to(server.block_hash(CHAIN_LENGTH)).await?;
    client.wait_for_persisted_block(CHAIN_LENGTH).await?;

    let attempt = client.inner.provider.database_provider_ro()?.snap_attempt()?.unwrap();
    assert!(attempt.is_verified());
    assert_eq!(attempt.pivot().number, CHAIN_LENGTH);
    let provider = client.rpc_provider();
    assert_eq!(provider.get_code_at(contract).await?, code);
    assert_eq!(provider.get_storage_at(contract, U256::ZERO).await?, U256::ONE);

    let next = server.mine([account.call(contract, Bytes::new()).await]).await?.ensure_success()?;
    client.import_payload(next.payload).await?;

    assert_eq!(provider.get_storage_at(contract, U256::ZERO).await?, U256::from(2));
    assert_eq!(provider.get_transaction_count(account.address()).await?, account.nonce());
    client.wait_for_persisted_block(CHAIN_LENGTH + 1).await?;
    assert_trie_consistency(&client.inner.provider)?;
    assert_eq!(client.inner.provider.database_provider_ro()?.snap_attempt()?, Some(attempt));
    Ok(())
}

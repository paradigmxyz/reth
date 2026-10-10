//! End-to-end sync scenarios with `--snap.v2`.
//!
//! - A fresh node snap-syncs the state at the finalized block of a 100 block Amsterdam chain from a
//!   serving peer, then executes the blocks above it with the staged pipeline, with persisted state
//!   and trie nodes matching the head state root.
//! - A fresh node syncs 100 finalized Prague blocks through the staged pipeline without starting a
//!   snap attempt.
//! - A snap-synced node downloads paginated contract storage and missing bytecode, stays unverified
//!   until the bytecode arrives, then reads the downloaded state and executes a new call that
//!   updates that storage.
//! - A completed snap sync survives a restart, preserves its verified attempt and state root, and
//!   imports another block without downloading snap state again.
//! - An interrupted storage download resumes after restart from its persisted cursor within the
//!   same snap attempt, then completes with consistent persisted state and trie nodes.
//! - An expired pivot advances through BALs without restarting the attempt or redownloading covered
//!   account ranges, then completes with the expected balances, nonce and state root.
//! - A download pauses when its only serving peer disconnects, keeps incomplete state unpublished,
//!   and completes the same attempt when the peer returns.
//! - A restarted download recovers from a reorged pivot using orphaned and canonical BALs, repairs
//!   orphan-only storage changes, and completes with the replacement chain's state root.
//! - RPC rejects state reads during an incomplete snap download and returns the expected state
//!   after verification and activation.

use crate::utils::advance_with_random_transactions;
use alloy_primitives::{bytes, keccak256, Bytes, B256, U256};
use alloy_provider::Provider;
use alloy_rpc_types_engine::{ForkchoiceState, PayloadStatusEnum};
use rand::{rngs::StdRng, SeedableRng};
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::{
    node::Finality,
    snap::SnapControl,
    trie::assert_trie_consistency,
    wait::{assert_holds_for, poll_until},
    E2ETestSetupBuilder, E2ETestSetupExt, NodeHelperType,
};
use reth_eth_wire_types::snap::SnapProtocolMessage;
use reth_network::PeersInfo;
use reth_node_ethereum::{snap::EthereumBackfill, EthereumNode};
use reth_provider::{
    DatabaseProviderFactory, HeaderProvider, MetadataProvider, StageCheckpointReader,
};
use reth_snap_sync::{SnapAccountStore, SnapAttemptStore, SnapStorageStore};
use reth_stages_types::StageId;
use reth_tasks::Runtime;
use std::time::Duration;

// Blocks the serving node builds: more than the backfill threshold, and few enough that it still
// serves the state of every block, see `SNAPSHOT_STATE_RETENTION`.
const CHAIN_LENGTH: u64 = 100;

// Finalized block a syncing node anchors its snap pivot to. It is below the head, so the staged
// pipeline executes the blocks above it on the downloaded state.
const FINALIZED: u64 = 80;

// A node on `fork` that serves snap/2 from the storage layout snap writes, and picks its backfill
// from `--snap.v2` like `reth node` does.
fn snap_setup(fork: EthereumHardfork, runtime: Runtime) -> E2ETestSetupBuilder<EthereumNode> {
    snap_node(fork).with_runtime(runtime)
}

// A setup without a shared runtime can also launch restartable nodes.
fn snap_node(fork: EthereumHardfork) -> E2ETestSetupBuilder<EthereumNode> {
    EthereumNode::test_setup_for(fork)
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

// Seeds enough storage to require pagination, then returns the slot-zero incrementer runtime.
fn incrementer_init_code() -> Bytes {
    let mut init_code = Vec::new();
    for slot in 1..=16 {
        init_code.extend_from_slice(&[0x60, slot, 0x60, slot, 0x55]);
    }
    init_code.extend_from_slice(&bytes!("675f546001015f55005f5260086018f3"));
    init_code.into()
}

#[tokio::test]
async fn a_fresh_node_snap_syncs_to_the_head() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let (server, client) = serving_and_syncing(EthereumHardfork::Amsterdam).await?;
    let finalized = server.block_hash(FINALIZED);

    client
        .sync_to_forkchoice(ForkchoiceState {
            head_block_hash: server.block_hash(CHAIN_LENGTH),
            safe_block_hash: finalized,
            finalized_block_hash: finalized,
        })
        .await?;

    // The state at the pivot came from snap, not from executing the chain.
    let attempt = client.inner.provider.database_provider_ro()?.snap_attempt()?.unwrap();
    let pivot = server.inner.provider.sealed_header(FINALIZED)?.unwrap();
    assert!(attempt.is_verified());
    assert_eq!(attempt.pivot(), pivot.num_hash());
    assert_eq!(attempt.state_root(), pivot.state_root);
    // The staged pipeline executed the blocks above the pivot on that state.
    client.wait_for_persisted_block(CHAIN_LENGTH).await?;
    assert_trie_consistency(&client.inner.provider)?;
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
    let mut account = wallet.account(0).with_fees(2_000_000_000, 1_000_000_000);

    // The runtime increments slot 0: PUSH0 SLOAD PUSH1 1 ADD PUSH0 SSTORE STOP.
    let code = bytes!("5f546001015f5500");
    let contract = account.next_contract_address();
    server
        .mine([account.deploy(incrementer_init_code()).gas_limit(8_000_000).await])
        .await?
        .ensure_success()?;
    server.mine([account.call(contract, Bytes::new()).await]).await?.ensure_success()?;
    server.advance_blocks(CHAIN_LENGTH - 2).await?;

    let control = SnapControl::default().with_response_bytes(256);
    let code_hash = keccak256(&code);
    let mut bytecode = control.pause_on(move |request| {
        matches!(request, SnapProtocolMessage::GetByteCodes(request) if request.hashes.contains(&code_hash))
    });
    let backfill_control = control.clone();
    let setup = snap_setup(EthereumHardfork::Amsterdam, runtime)
        .with_backfill(move |config| backfill_control.backfill(EthereumBackfill::new(config)))
        .with_tree_config_modifier(|config| {
            config.with_persistence_threshold(0).with_memory_block_buffer_target(0)
        });
    let client = syncing_client(&mut server, setup).await?;
    let status = client
        .engine
        .forkchoice_updated(ForkchoiceState::same_hash(server.block_hash(CHAIN_LENGTH)))
        .await?;
    assert_eq!(status.payload_status.status, PayloadStatusEnum::Syncing);
    bytecode.reached().await?;
    let pending = client.inner.provider.database_provider_ro()?.snap_attempt()?.unwrap();
    assert!(!pending.is_verified());
    assert_eq!(
        client.inner.provider.get_stage_checkpoint(StageId::Finish)?.unwrap().block_number,
        0
    );
    let contract_hash = keccak256(contract);
    assert!(control.requests().iter().any(|request| matches!(request,
        SnapProtocolMessage::GetStorageRanges(request)
        if request.account_hashes == [contract_hash] && request.starting_hash.unwrap_or(B256::ZERO) != B256::ZERO
    )), "contract storage must require a continuation request");
    bytecode.release();
    client.sync_to(server.block_hash(CHAIN_LENGTH)).await?;
    client.wait_for_persisted_block(CHAIN_LENGTH).await?;

    let attempt = client.inner.provider.database_provider_ro()?.snap_attempt()?.unwrap();
    assert!(attempt.is_verified());
    assert_eq!(attempt.id(), pending.id());
    assert_eq!(attempt.pivot().number, CHAIN_LENGTH);
    let provider = client.rpc_provider();
    assert_eq!(provider.get_code_at(contract).await?, code);
    assert_eq!(provider.get_storage_at(contract, U256::ZERO).await?, U256::ONE);
    for slot in 1..=16 {
        assert_eq!(provider.get_storage_at(contract, U256::from(slot)).await?, U256::from(slot));
    }

    let next = server.mine([account.call(contract, Bytes::new()).await]).await?.ensure_success()?;
    client.import_payload(next.payload).await?;

    assert_eq!(provider.get_storage_at(contract, U256::ZERO).await?, U256::from(2));
    assert_eq!(provider.get_transaction_count(account.address()).await?, account.nonce());
    client.wait_for_persisted_block(CHAIN_LENGTH + 1).await?;
    assert_trie_consistency(&client.inner.provider)?;
    assert_eq!(client.inner.provider.database_provider_ro()?.snap_attempt()?, Some(attempt));
    Ok(())
}

#[tokio::test]
async fn a_completed_snap_sync_survives_restart() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let runtime = Runtime::test();
    let (mut server, _) = snap_setup(EthereumHardfork::Amsterdam, runtime).build_single().await?;
    server.advance_blocks(CHAIN_LENGTH).await?;
    let control = SnapControl::default();
    let backfill_control = control.clone();
    let setup = snap_node(EthereumHardfork::Amsterdam)
        .with_restartable_nodes()
        .with_backfill(move |config| backfill_control.backfill(EthereumBackfill::new(config)))
        .with_tree_config_modifier(|config| {
            config.with_persistence_threshold(0).with_memory_block_buffer_target(0)
        });
    let client = syncing_client(&mut server, setup).await?;
    client.sync_to(server.block_hash(CHAIN_LENGTH)).await?;
    client.wait_for_persisted_block(CHAIN_LENGTH).await?;
    let attempt = client.inner.provider.database_provider_ro()?.snap_attempt()?.unwrap();
    assert!(attempt.is_verified());
    let requests = control.requests();

    let client = client.restart().await?;
    assert_eq!(client.block_hash(CHAIN_LENGTH), server.block_hash(CHAIN_LENGTH));
    assert_eq!(client.inner.provider.database_provider_ro()?.snap_attempt()?, Some(attempt));
    assert_trie_consistency(&client.inner.provider)?;
    let next = server.mine([]).await?;
    client.import_payload(next.payload).await?;
    client.wait_for_persisted_block(CHAIN_LENGTH + 1).await?;
    assert_trie_consistency(&client.inner.provider)?;
    assert_eq!(control.requests(), requests, "verified state must not be downloaded again");
    Ok(())
}

#[tokio::test]
async fn partial_storage_resumes_after_restart() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let (mut server, wallet) = snap_node(EthereumHardfork::Amsterdam).build_single().await?;
    let mut account = wallet.account(0).with_fees(2_000_000_000, 1_000_000_000);
    server
        .mine([account.deploy(incrementer_init_code()).gas_limit(8_000_000).await])
        .await?
        .ensure_success()?;
    server.advance_blocks(CHAIN_LENGTH - 1).await?;
    let control = SnapControl::default().with_response_bytes(256);
    // Stop after one page has committed, before fetching the continuation.
    let mut continuation = control.pause_on(|request| {
        matches!(request,
            SnapProtocolMessage::GetStorageRanges(request)
            if request.starting_hash.unwrap_or(B256::ZERO) != B256::ZERO
        )
    });
    let backfill_control = control.clone();
    let setup = snap_node(EthereumHardfork::Amsterdam)
        .with_restartable_nodes()
        .with_backfill(move |config| backfill_control.backfill(EthereumBackfill::new(config)));
    let client = syncing_client(&mut server, setup).await?;
    client
        .engine
        .forkchoice_updated(ForkchoiceState::same_hash(server.block_hash(CHAIN_LENGTH)))
        .await?;
    continuation.reached().await?;
    let (write, origin, account, resume_at) = {
        let provider = client.inner.provider.database_provider_ro()?;
        let write = provider.active_snap_write()?.unwrap();
        let origin = provider.account_coverage(write)?.unwrap().next().unwrap();
        let requests = control.requests();
        let SnapProtocolMessage::GetStorageRanges(request) = requests.last().unwrap() else {
            panic!("paused storage request")
        };
        let account = request.account_hashes[0];
        let resume_at = request.starting_hash.unwrap_or(B256::ZERO);
        assert_eq!(provider.storage_progress(write, origin)?.resume_at(account), Some(resume_at));
        (write, origin, account, resume_at)
    };
    let stopped = client.stop().await?;
    let before_restart = control.requests().len();
    let mut resumed = control.pause_on(move |request| {
        matches!(request,
            SnapProtocolMessage::GetStorageRanges(request) if request.account_hashes == [account]
        )
    });
    let mut client = stopped.start().await?;
    {
        let provider = client.inner.provider.database_provider_ro()?;
        assert_eq!(provider.active_snap_write()?, Some(write));
        assert_eq!(provider.storage_progress(write, origin)?.resume_at(account), Some(resume_at));
    }
    client.connect(&mut server).await;
    client
        .engine
        .forkchoice_updated(ForkchoiceState::same_hash(server.block_hash(CHAIN_LENGTH)))
        .await?;
    resumed.reached().await?;
    let requests = control.requests();
    let resumed_request = requests[before_restart..]
        .iter()
        .find_map(|request| match request {
            SnapProtocolMessage::GetStorageRanges(request)
                if request.account_hashes == [account] =>
            {
                Some(request)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(resumed_request.starting_hash.unwrap_or(B256::ZERO), resume_at);
    resumed.release();
    client.sync_to(server.block_hash(CHAIN_LENGTH)).await?;
    client.wait_for_persisted_block(CHAIN_LENGTH).await?;
    let attempt = client.inner.provider.database_provider_ro()?.snap_attempt()?.unwrap();
    assert_eq!(attempt.id(), write.attempt());
    assert!(attempt.is_verified());
    assert_trie_consistency(&client.inner.provider)?;
    Ok(())
}

#[tokio::test]
async fn an_expired_pivot_advances_with_bals_without_restarting() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let (mut server, wallet) = snap_node(EthereumHardfork::Amsterdam)
        .with_tree_config_modifier(|config| {
            config.with_persistence_threshold(0).with_memory_block_buffer_target(0)
        })
        .build_single()
        .await?;
    // An early account is already persisted when the next range is paused.
    let mut account = (0..20)
        .map(|index| wallet.account(index))
        .min_by_key(|account| keccak256(account.address()))
        .unwrap();
    let recipient = (0..20)
        .map(|index| wallet.account(index))
        .max_by_key(|account| keccak256(account.address()))
        .unwrap()
        .address();
    server.advance_blocks(FINALIZED).await?;
    // Change the root immediately after the pivot: empty blocks would keep its root servable.
    server.mine([account.transfer(recipient, U256::ONE).await]).await?.ensure_success()?;
    server.advance_blocks(CHAIN_LENGTH - FINALIZED - 1).await?;
    let account_hash = keccak256(account.address());
    let control = SnapControl::default().with_response_bytes(256);
    let mut downloaded = control.pause_on(move |request| {
        matches!(request,
            SnapProtocolMessage::GetAccountRange(request) if request.starting_hash > account_hash
        )
    });
    let backfill_control = control.clone();
    let setup = snap_node(EthereumHardfork::Amsterdam)
        .with_backfill(move |config| backfill_control.backfill(EthereumBackfill::new(config)));
    let client = syncing_client(&mut server, setup).await?;
    let finalized = server.block_hash(FINALIZED);
    client
        .engine
        .forkchoice_updated(ForkchoiceState {
            head_block_hash: server.block_hash(CHAIN_LENGTH),
            safe_block_hash: finalized,
            finalized_block_hash: finalized,
        })
        .await?;
    downloaded.reached().await?;
    let (attempt, origin) = {
        let provider = client.inner.provider.database_provider_ro()?;
        let attempt = provider.snap_attempt()?.unwrap();
        assert_eq!(attempt.pivot().number, FINALIZED);
        let write = provider.active_snap_write()?.unwrap();
        let origin = provider.account_coverage(write)?.unwrap().next().unwrap();
        assert!(origin > account_hash);
        assert!(origin <= keccak256(recipient));
        (attempt, origin)
    };

    // The peer drops the old pivot state from its retention window while BALs remain served.
    server.mine([account.transfer(recipient, U256::ONE).await]).await?.ensure_success()?;
    let target = 210;
    server.advance_blocks(target - CHAIN_LENGTH - 1).await?;
    server.wait_for_persisted_block(target).await?;
    let before_advance = control.requests().len();
    let mut bals =
        control.pause_on(|request| matches!(request, SnapProtocolMessage::GetBlockAccessLists(_)));
    client.engine.forkchoice_updated(ForkchoiceState::same_hash(server.block_hash(target))).await?;
    downloaded.release();
    bals.reached().await?;
    let advanced = client.inner.provider.database_provider_ro()?.snap_attempt()?.unwrap();
    assert_eq!(advanced.id(), attempt.id());
    assert_eq!(advanced.pivot().number, target);
    assert!(!advanced.is_verified());
    bals.release();
    client.sync_to(server.block_hash(target)).await?;
    client.wait_for_persisted_block(target).await?;

    let requests = control.requests();
    let requested_blocks: Vec<_> = requests[before_advance..]
        .iter()
        .filter_map(|request| match request {
            SnapProtocolMessage::GetBlockAccessLists(request) => {
                Some(request.block_hashes.iter().copied())
            }
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(
        requested_blocks,
        ((FINALIZED + 1)..=target).map(|number| server.block_hash(number)).collect::<Vec<_>>()
    );
    assert!(
        requests[before_advance..]
            .iter()
            .filter_map(|request| match request {
                SnapProtocolMessage::GetAccountRange(request) => Some(request.starting_hash),
                _ => None,
            })
            .all(|starting_hash| starting_hash >= origin),
        "downloaded account ranges must be retained"
    );
    let completed = client.inner.provider.database_provider_ro()?.snap_attempt()?.unwrap();
    assert_eq!(completed.id(), attempt.id());
    assert_eq!(completed.pivot(), advanced.pivot());
    assert!(completed.is_verified());
    assert_eq!(
        client.rpc_provider().get_transaction_count(account.address()).await?,
        account.nonce()
    );
    assert_eq!(
        client.rpc_provider().get_balance(account.address()).await?,
        server.rpc_provider().get_balance(account.address()).await?
    );
    assert_eq!(
        client.rpc_provider().get_balance(recipient).await?,
        server.rpc_provider().get_balance(recipient).await?
    );
    assert_trie_consistency(&client.inner.provider)?;
    Ok(())
}

#[tokio::test]
async fn a_download_resumes_when_its_only_peer_returns() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let (mut server, _) =
        snap_node(EthereumHardfork::Amsterdam).with_restartable_nodes().build_single().await?;
    server.advance_blocks(CHAIN_LENGTH).await?;
    let head = server.block_hash(CHAIN_LENGTH);
    let control = SnapControl::default().with_response_bytes(256);
    let mut continuation = control.pause_on(|request| {
        matches!(request,
            SnapProtocolMessage::GetAccountRange(request) if request.starting_hash != B256::ZERO
        )
    });
    let backfill_control = control.clone();
    let setup = snap_node(EthereumHardfork::Amsterdam)
        .with_backfill(move |config| backfill_control.backfill(EthereumBackfill::new(config)));
    let mut client = syncing_client(&mut server, setup).await?;
    client.engine.forkchoice_updated(ForkchoiceState::same_hash(head)).await?;
    continuation.reached().await?;
    let attempt = client.inner.provider.database_provider_ro()?.snap_attempt()?.unwrap();
    let stopped = server.stop().await?;
    poll_until("the only serving peer to disconnect", || async {
        Ok((client.inner.network.num_connected_peers() == 0).then_some(()))
    })
    .await?;
    continuation.release();
    assert_holds_for(
        Duration::from_secs(1),
        "the incomplete attempt to remain unpublished without peers",
        || async {
            let current = client.inner.provider.database_provider_ro()?.snap_attempt()?.unwrap();
            Ok(current.id() == attempt.id() &&
                !current.is_verified() &&
                client
                    .inner
                    .provider
                    .get_stage_checkpoint(StageId::Finish)?
                    .unwrap()
                    .block_number ==
                    0)
        },
    )
    .await?;
    let mut server = stopped.start().await?;
    server.connect(&mut client).await;
    client.sync_to(head).await?;
    client.wait_for_persisted_block(CHAIN_LENGTH).await?;
    let completed = client.inner.provider.database_provider_ro()?.snap_attempt()?.unwrap();
    assert_eq!(completed.id(), attempt.id());
    assert!(completed.is_verified());
    assert_trie_consistency(&client.inner.provider)?;
    Ok(())
}

#[tokio::test]
async fn a_reorged_pivot_repairs_orphaned_storage() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let runtime = Runtime::test();
    let (mut server, wallet) =
        snap_setup(EthereumHardfork::Amsterdam, runtime.clone()).build_single().await?;
    let (mut fork, _) = snap_setup(EthereumHardfork::Amsterdam, runtime).build_single().await?;
    server.set_finality(Finality::Keep);
    fork.set_finality(Finality::Keep);
    // Choose a contract early in the trie so some account ranges remain after it is downloaded.
    let mut account = (0..20)
        .map(|index| wallet.account(index))
        .min_by_key(|account| keccak256(account.next_contract_address()))
        .unwrap();
    let contract = account.next_contract_address();
    let contract_hash = keccak256(contract);
    // Store calldata word 1 at the key in word 0: PUSH1 32 CALLDATALOAD PUSH0 CALLDATALOAD SSTORE
    // STOP.
    let deployed = server
        .mine([account.deploy(bytes!("666020355f3555005f5260076019f3")).await])
        .await?
        .ensure_success()?;
    fork.import_payload(deployed.payload).await?;
    for payload in server.advance_blocks(31).await? {
        fork.import_payload(payload).await?;
    }
    fork.wait_for_pool_head(server.block_hash(32)).await?;
    fork.set_next_payload_timestamp(
        server.inner.provider.sealed_header(32)?.unwrap().timestamp + 1,
    )?;
    let mut fork_account = account.clone();
    let orphaned = server
        .mine([
            account.call(contract, storage_input(0, 11)).await,
            account.call(contract, storage_input(1, 7)).await,
        ])
        .await?
        .ensure_success()?;
    server.advance_blocks(CHAIN_LENGTH - 33).await?;
    // No finalized pivot is supplied, so the initial pivot is HEAD - 64 = 36.
    let orphaned_hashes: Vec<_> = (33..=36).map(|number| server.block_hash(number)).collect();
    let control = SnapControl::default().with_response_bytes(256);
    let mut downloaded = control.pause_on(move |request| {
        matches!(request,
            SnapProtocolMessage::GetAccountRange(request) if request.starting_hash > contract_hash
        )
    });
    let backfill_control = control.clone();
    let setup = snap_node(EthereumHardfork::Amsterdam)
        .with_restartable_nodes()
        .with_backfill(move |config| backfill_control.backfill(EthereumBackfill::new(config)));
    let client = syncing_client(&mut server, setup).await?;
    client.update_optimistic_forkchoice(server.block_hash(CHAIN_LENGTH)).await?;
    downloaded.reached().await?;
    let attempt = client.inner.provider.database_provider_ro()?.snap_attempt()?.unwrap();
    assert_eq!(attempt.pivot().number, 36);
    let stopped = client.stop().await?;

    // Both branches change slot 0, but only the orphan changes slot 1 and only the new fork
    // changes slot 2. Replaying the new branch alone cannot remove the orphan-only value.
    let replacement = fork
        .mine([
            fork_account.call(contract, storage_input(0, 22)).await,
            fork_account.call(contract, storage_input(2, 9)).await,
        ])
        .await?
        .ensure_success()?;
    assert_ne!(replacement.payload.block().hash(), orphaned.payload.block().hash());
    server.import_payload(replacement.payload).await?;
    for payload in fork.advance_blocks(101 - 33).await? {
        server.import_payload(payload).await?;
    }
    let before_restart = control.requests().len();
    let mut recovery =
        control.pause_on(|request| matches!(request, SnapProtocolMessage::GetBlockAccessLists(_)));
    let mut client = stopped.start().await?;
    client.connect(&mut server).await;
    client.update_optimistic_forkchoice(server.block_hash(101)).await?;
    poll_until("canonical headers to reach the replacement head", || async {
        Ok(client.inner.provider.sealed_header(101)?)
    })
    .await?;
    let head = client.inner.provider.sealed_header(101)?.unwrap();
    let parent = client.inner.provider.sealed_header(100)?.unwrap();
    assert_eq!(
        head.parent_hash,
        parent.hash(),
        "header catch-up must replace the old branch before recovering snap state"
    );
    recovery.reached().await?;
    assert_eq!(
        client.inner.provider.database_provider_ro()?.snap_attempt()?.unwrap().id(),
        attempt.id()
    );
    recovery.release();
    client
        .sync_to_forkchoice(ForkchoiceState {
            head_block_hash: server.block_hash(101),
            safe_block_hash: B256::ZERO,
            finalized_block_hash: B256::ZERO,
        })
        .await?;
    client.wait_for_persisted_block(101).await?;
    let requests = control.requests();
    let requested_blocks: Vec<_> = requests[before_restart..]
        .iter()
        .filter_map(|request| match request {
            SnapProtocolMessage::GetBlockAccessLists(request) => {
                Some(request.block_hashes.iter().copied())
            }
            _ => None,
        })
        .flatten()
        .collect();
    let expected: Vec<_> = orphaned_hashes
        .into_iter()
        .chain((33..=37).map(|number| server.block_hash(number)))
        .collect();
    assert_eq!(requested_blocks, expected);
    assert!(requests[before_restart..].iter().any(|request| matches!(request,
        SnapProtocolMessage::GetAccountRange(request) if request.starting_hash == contract_hash && request.limit_hash == contract_hash
    )), "the downloaded contract must be repaired at the new pivot");
    let completed = client.inner.provider.database_provider_ro()?.snap_attempt()?.unwrap();
    assert_eq!(completed.id(), attempt.id());
    assert_eq!(completed.pivot().number, 37);
    assert!(completed.is_verified());
    for (slot, value) in [(0, 22), (1, 0), (2, 9)] {
        assert_eq!(
            client.rpc_provider().get_storage_at(contract, U256::from(slot)).await?,
            U256::from(value)
        );
    }
    assert_trie_consistency(&client.inner.provider)?;
    Ok(())
}

// Calldata for the storage writer used by the reorg scenario.
fn storage_input(slot: u64, value: u64) -> Bytes {
    [U256::from(slot).to_be_bytes::<32>(), U256::from(value).to_be_bytes::<32>()].concat().into()
}

#[tokio::test]
async fn incomplete_snap_state_is_not_served_over_rpc() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let (mut server, wallet) = snap_node(EthereumHardfork::Amsterdam).build_single().await?;
    let mut account = (0..20)
        .map(|index| wallet.account(index))
        .min_by_key(|account| keccak256(account.address()))
        .unwrap();
    server
        .mine([account.transfer(wallet.account(19).address(), U256::ONE).await])
        .await?
        .ensure_success()?;
    server.advance_blocks(CHAIN_LENGTH - 1).await?;
    let account_hash = keccak256(account.address());
    let control = SnapControl::default().with_response_bytes(256);
    let mut downloaded = control.pause_on(move |request| {
        matches!(request,
            SnapProtocolMessage::GetAccountRange(request) if request.starting_hash > account_hash
        )
    });
    let backfill_control = control.clone();
    let setup = snap_node(EthereumHardfork::Amsterdam)
        .with_backfill(move |config| backfill_control.backfill(EthereumBackfill::new(config)));
    let client = syncing_client(&mut server, setup).await?;
    client
        .engine
        .forkchoice_updated(ForkchoiceState::same_hash(server.block_hash(CHAIN_LENGTH)))
        .await?;
    downloaded.reached().await?;
    let attempt = client.inner.provider.database_provider_ro()?.snap_attempt()?.unwrap();
    assert!(!attempt.is_verified());
    assert_eq!(
        client.inner.provider.get_stage_checkpoint(StageId::Finish)?.unwrap().block_number,
        0
    );
    // A downloaded account is not trustworthy as canonical RPC state until the whole root has
    // been verified. Returning its balance here exposes a mixture of downloaded and absent state.
    let error = client
        .rpc_provider()
        .get_balance(account.address())
        .await
        .expect_err("RPC must reject state reads until the snap root has been verified");
    let error = error.as_error_resp().expect("expected a JSON-RPC error response");
    assert_eq!(error.code, -32603);
    downloaded.release();
    client.sync_to(server.block_hash(CHAIN_LENGTH)).await?;
    assert_eq!(
        client.rpc_provider().get_balance(account.address()).await?,
        server.rpc_provider().get_balance(account.address()).await?
    );
    Ok(())
}

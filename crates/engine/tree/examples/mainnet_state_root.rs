//! Replay consecutive blocks from an offline Ethereum mainnet snapshot without writing it.
//!
//! `cargo run -p reth-engine-tree --example mainnet_state_root -- DATADIR FIRST [COUNT] [WORKERS]`
//! Add `--features fuzz` for deterministic scheduling of the blocking state-root/proof path.
//! Tokio stays native; this is not a whole-node simulation or randomized schedule campaign.
//! Hash seeds are not controlled; matching roots do not imply identical internal batching.

#![forbid(unsafe_code)]

use eyre::{ensure, eyre, OptionExt, Result, WrapErr};
use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
use reth_chainspec::{EthChainSpec, MAINNET};
use reth_engine_primitives::TreeConfig;
use reth_engine_tree::tree::state_root_strategy::{
    publish_sparse_trie, DefaultStateRootStrategy, StateRootTaskOptions,
};
use reth_ethereum_primitives::EthPrimitives;
use reth_evm::{database::StateProviderDatabase, ConfigureEvm, Executor, OnStateHook};
use reth_evm_ethereum::EthEvmConfig;
use reth_node_ethereum::EthereumNode;
use reth_provider::{
    providers::ReadOnlyConfig, BlockReader, DatabaseProviderROFactory, HashedPostStateProvider,
    HeaderProvider, StateProvider, StateRootProvider, TransactionVariant,
};
use reth_storage_overlay::{OverlayManager, OverlayStateProviderFactory};
use reth_tasks::{
    executor::{time::Duration, Runtime},
    runtime::{DEFAULT_MAX_BLOCKING_TASKS, DEFAULT_RESERVED_CPU_CORES},
    RayonConfig, RuntimeBuilder, RuntimeConfig, TokioConfig,
};
use std::{
    env,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use tokio::runtime::{Builder, Handle};

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let datadir = PathBuf::from(args.next().ok_or_eyre("expected DATADIR")?);
    let first = args.next().ok_or_eyre("expected FIRST block number")?.parse::<u64>()?;
    let count = args.next().map(|value| value.parse::<u64>()).transpose()?.unwrap_or(2);
    let workers = args.next().map(|value| value.parse::<usize>()).transpose()?.unwrap_or(2);
    ensure!(args.next().is_none(), "expected DATADIR FIRST [COUNT] [WORKERS]");
    ensure!(first > 0, "FIRST must have a parent");
    ensure!((2..=32).contains(&count), "COUNT must be 2..=32 for cold and warm coverage");
    ensure!((1..=16).contains(&workers), "WORKERS must be 1..=16");
    let end = first.checked_add(count).ok_or_eyre("block range overflow")?;

    // Native Tokio is constructed outside the selected blocking executor's managed scope.
    let tokio = Builder::new_multi_thread().worker_threads(1).enable_all().build()?;
    let tokio_handle = tokio.handle().clone();
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    recorder.install().wrap_err("metrics recorder")?;
    Runtime::new().run(move || replay(datadir, first, end, workers, tokio_handle, snapshotter))
}

fn replay(
    datadir: PathBuf,
    first: u64,
    end: u64,
    workers: usize,
    tokio_handle: Handle,
    snapshotter: Snapshotter,
) -> Result<()> {
    let runtime = RuntimeBuilder::new(
        RuntimeConfig::default().with_tokio(TokioConfig::existing_handle(tokio_handle)).with_rayon(
            RayonConfig {
                cpu_threads: Some(workers),
                rpc_threads: Some(1),
                storage_threads: Some(1),
                proof_storage_worker_threads: Some(workers),
                proof_account_worker_threads: Some(workers),
                prewarming_threads: Some(1),
                bal_streaming_threads: Some(1),
                state_trie_overlay_worker_threads: Some(workers),
                reserved_cpu_cores: DEFAULT_RESERVED_CPU_CORES,
                max_blocking_tasks: DEFAULT_MAX_BLOCKING_TASKS,
            },
        ),
    )
    .build()?;
    // Watchers and long-read monitoring would otherwise introduce background activity.
    let factory = EthereumNode::provider_factory_builder().open_read_only(
        MAINNET.clone(),
        ReadOnlyConfig::from_datadir(datadir).no_watch().disable_long_read_transaction_safety(),
        runtime.clone(),
    )?;
    let genesis = factory.sealed_header(0)?.ok_or_eyre("missing genesis header")?;
    ensure!(genesis.hash() == MAINNET.genesis_hash(), "snapshot is not Ethereum mainnet");
    let overlay = OverlayManager::<EthPrimitives>::new(runtime.state_trie_overlay_worker_pool());
    let evm = EthEvmConfig::mainnet();
    let config = TreeConfig::default();
    let strategy = DefaultStateRootStrategy::default();
    for number in first..end {
        let block = Arc::new(
            factory
                .recovered_block(number.into(), TransactionVariant::NoHash)?
                .ok_or_else(|| eyre!("missing block {number}"))?,
        );
        let parent = factory
            .sealed_header(number - 1)?
            .ok_or_else(|| eyre!("missing parent of {number}"))?;
        ensure!(block.parent_hash == parent.hash(), "parent hash mismatch at {number}");
        ensure!(!block.body().transactions.is_empty(), "empty block {number} is not coverage");
        let preserved = overlay.take_sparse_trie();
        ensure!(preserved.is_some() == (number != first), "missing warm trie at {number}");
        let provider_factory = OverlayStateProviderFactory::new(
            factory.clone(),
            overlay.overlay_builder(parent.hash()),
        );
        let (mut handle, pending) = strategy.spawn_state_root(
            &runtime,
            provider_factory.clone(),
            StateRootTaskOptions::<EthPrimitives> {
                parent_header: parent.clone(),
                preserved_sparse_trie: preserved,
                transaction_count: Some(block.transaction_count()),
                gas_used: Some(block.gas_used),
                config: &config,
                pending_sparse_trie_prune_blocks: None,
            },
        );
        drop(handle.take_hint_stream());
        let mut hook = handle.take_execution_hook();
        let updates = Arc::new(AtomicUsize::new(0));
        let hook_updates = Arc::clone(&updates);
        let provider = provider_factory.database_provider_ro()?;
        let output = evm
            .executor(StateProviderDatabase((&provider).into_evm_state_provider()))
            .execute_with_state_hook(&block, move |state| {
                hook_updates.fetch_add(1, Ordering::Relaxed);
                hook.on_state(state);
            })?;
        ensure!(updates.load(Ordering::Relaxed) > 0, "no execution updates at {number}");
        ensure!(output.result.gas_used == block.gas_used, "gas mismatch at {number}");
        let hashed = provider.hashed_post_state(&output.state)?;
        ensure!(!hashed.accounts.is_empty(), "no changed accounts at {number}");
        ensure!(!hashed.storages.is_empty(), "no changed storage at {number}");
        let parallel = handle.take_state_root_rx().recv_timeout(Duration::from_secs(600))??;
        let reveals =
            snapshotter
                .snapshot()
                .into_vec()
                .into_iter()
                .find_map(|(key, _, _, value)| {
                    if key.key().name() ==
                        "tree.root.sparse_trie_reveal_multiproof_duration_histogram" &&
                        let DebugValue::Histogram(samples) = value
                    {
                        Some(samples.len())
                    } else {
                        None
                    }
                })
                .unwrap_or(0);
        ensure!(number != first || reveals > 0, "cold pipeline never received proofs at {number}");
        let (serial, _) = provider.state_root_with_updates(hashed.clone())?;
        ensure!(parallel.state_root == block.state_root, "parallel/header mismatch at {number}");
        ensure!(serial == block.state_root, "serial/header mismatch at {number}");
        ensure!(parallel.state_root != parent.state_root, "unchanged root at {number}");
        // Transaction deltas can contain writes that cancel over the block; compare state effects.
        let (stream_serial, _) =
            provider.state_root_with_updates(parallel.hashed_state.as_ref().clone())?;
        ensure!(
            stream_serial == serial,
            "stream/bundle state-effect mismatch at {number}: stream={stream_serial}, bundle={serial}"
        );
        publish_sparse_trie(&pending, &overlay, block.hash(), parallel.state_root);
        println!(
            "PASS block={number} hash={} mode={} workers={workers} txs={} gas={} updates={} accounts={} storages={} stream_accounts={} stream_storages={} same_delta_maps={} proof_reveals={reveals} root={serial}",
            block.hash(),
            if number == first { "cold" } else { "warm" },
            block.transaction_count(),
            block.gas_used,
            updates.load(Ordering::Relaxed),
            hashed.accounts.len(),
            hashed.storages.len(),
            parallel.hashed_state.accounts.len(),
            parallel.hashed_state.storages.len(),
            *parallel.hashed_state == hashed,
        );
    }
    // Join the sparse task's handoff/pruning before leaving the managed scope.
    let preserved = overlay.take_sparse_trie().ok_or_eyre("last trie not published")?;
    let last = factory.sealed_header(end - 1)?.ok_or_eyre("missing last header")?;
    ensure!(preserved.into_trie_for(last.hash())?.is_some(), "last trie not reusable");
    drop(overlay);
    drop(factory);
    for name in ["sparse-trie", "trie-hashing", "storage-workers", "account-workers", "drop"] {
        runtime.spawn_blocking_named(name, || {}).get();
    }
    drop(runtime);
    Ok(())
}

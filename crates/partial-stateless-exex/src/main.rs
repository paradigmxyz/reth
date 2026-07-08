//! Partial Statelessness ExEx — reth Execution Extension that maintains a
//! network-level state cache and reports witness requirements per block.
//!
//! Run with:
//!   cargo run -p partial-stateless-exex -- node --chain mainnet --datadir /path/to/data
//!
//! This ExEx subscribes to canonical chain commits and:
//! 1. Extracts `BlockAccessedState` via EVM simulation of each block
//! 2. Updates the `NetworkStateCache` with the accessed state
//! 3. Computes and logs cache miss ratio (= witness requirement)
//! 4. Computes actual Merkle proof (witness) size for cache-missed state

mod sidecar_create;
mod sidecar_io;
mod sidecar_reexec;
mod sidecar_verify;

use alloy_primitives::B256;
use alloy_rlp::Encodable;
use futures::TryStreamExt;
use partial_stateless::{
    network_cache::NetworkStateCache,
    persistence::{load_from_file, save_to_file},
    policy::LastNBlocksPolicy,
};
use reth_ethereum::{
    chainspec::EthChainSpec,
    exex::{ExExContext, ExExEvent, ExExNotification},
    node::{
        api::{FullNodeComponents, NodeTypes},
        builder::NodeHandleFor,
        EthereumNode,
    },
    provider::StateProviderFactory,
    EthPrimitives,
};
use reth_provider::{BlockIdReader, HeaderProvider};
use sidecar_create::{create_sidecar_for_block, BuilderBlockReport, BuilderOptions};
use sidecar_reexec::SidecarReexecLimits;
use sidecar_verify::verify_live_sidecar;
use std::path::PathBuf;
use std::time::Duration;
use tracing::{info, warn};

/// Configuration for the partial statelessness cache.
struct CacheConfig {
    /// Window size for account eviction policy (in blocks).
    account_window: u64,
    /// Window size for storage/code eviction policy (in blocks).
    storage_window: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self { account_window: 60, storage_window: 30 }
    }
}

impl CacheConfig {
    fn new_cache(&self) -> NetworkStateCache {
        NetworkStateCache::new(
            Box::new(LastNBlocksPolicy::new(self.account_window)),
            Box::new(LastNBlocksPolicy::new(self.storage_window)),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidecarRole {
    Builder,
    BuilderVerifier,
    Verifier,
}

impl SidecarRole {
    fn from_env() -> Self {
        let Some(value) = std::env::var("PS_SIDECAR_ROLE").ok() else {
            return Self::Builder;
        };
        let normalized = value.to_ascii_lowercase().replace('_', "-");

        match normalized.as_str() {
            "builder" | "build" => Self::Builder,
            "builder-verifier" | "both" | "test" => Self::BuilderVerifier,
            "verifier" | "verify" | "client" => Self::Verifier,
            other => {
                warn!(
                    target: "partial_stateless",
                    value = other,
                    "Unknown PS_SIDECAR_ROLE; falling back to builder"
                );
                Self::Builder
            }
        }
    }
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "on"))
        .unwrap_or(false)
}

fn env_millis(name: &str, default: u64) -> Duration {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or_else(|| Duration::from_millis(default))
}

fn configured_sidecar_dir() -> PathBuf {
    std::env::var_os("PS_SIDECAR_DIR").map(PathBuf::from).unwrap_or_else(|| {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join("sidecar")
    })
}

/// The ExEx function that processes chain notifications and maintains the cache.
async fn partial_stateless_exex<
    Node: FullNodeComponents<Types: NodeTypes<Primitives = EthPrimitives>>,
>(
    mut ctx: ExExContext<Node>,
    config: CacheConfig,
) -> eyre::Result<()> {
    // Resolve the cache file path: datadir/partial_stateless_cache.bin
    let cache_dir = ctx.config.datadir.clone().resolve_datadir(ctx.config.chain.chain());
    let cache_path = cache_dir.as_ref().join("partial_stateless_cache.bin");

    // Cache coherence guarantee: the LastNBlocksPolicy is fully deterministic —
    // given the same canonical chain and window parameters, all peers converge
    // to the same cache state. Loading from disk and replaying missed blocks
    // during sync produces an identical result to continuous operation.
    let mut cache = if cache_path.exists() {
        match load_from_file(
            &cache_path,
            Box::new(LastNBlocksPolicy::new(config.account_window)),
            Box::new(LastNBlocksPolicy::new(config.storage_window)),
        ) {
            Ok(loaded_cache) => {
                let cache_block = loaded_cache.current_block();
                let head_block = ctx.head.number;

                // Validation: Gap Tolerance based on config.account_window
                let max_allowed_gap = config.account_window;
                if cache_block <= head_block && head_block - cache_block <= max_allowed_gap {
                    info!(
                        target: "partial_stateless",
                        cache_block = cache_block,
                        head_block = head_block,
                        gap = head_block - cache_block,
                        "Warm state cache loaded successfully from disk. Continuing sync..."
                    );
                    loaded_cache
                } else {
                    warn!(
                            target: "partial_stateless",
                            cache_block = cache_block,
                            head_block = head_block,
                            max_allowed_gap = max_allowed_gap,
                            "Cache file block state is too far from head block or in the future. Starting with cold cache."
                    );
                    config.new_cache()
                }
            }
            Err(e) => {
                warn!(
                    target: "partial_stateless",
                    error = %e,
                    "Failed to load cache file from disk. Starting with cold cache."
                );
                config.new_cache()
            }
        }
    } else {
        info!(
            target: "partial_stateless",
            "No existing cache file found at {}. Starting with cold cache.",
            cache_path.display()
        );
        config.new_cache()
    };

    let sidecar_role = SidecarRole::from_env();
    let sidecar_dir = configured_sidecar_dir();
    let verifier_wait = env_millis("PS_SIDECAR_VERIFIER_WAIT_MS", 2_000);

    info!(
        target: "partial_stateless",
        account_window = config.account_window,
        storage_window = config.storage_window,
        cache_path = %cache_path.display(),
        sidecar_role = ?sidecar_role,
        sidecar_dir = %sidecar_dir.display(),
        verifier_wait_ms = verifier_wait.as_millis(),
        "Partial Stateless ExEx started — monitoring cache state per block"
    );

    // Optional reproducible-dataset capture: when `PS_CAPTURE_DIR` is set, dump the
    // per-block `BlockAccessedState` (the only cache input) so the cache-window
    // benchmark can replay a fixed range offline, with no node or EVM. This reuses
    // the exact execution path the live system uses, so the dataset is faithful.
    let capture_dir = std::env::var("PS_CAPTURE_DIR").ok().map(std::path::PathBuf::from);
    if let Some(dir) = &capture_dir {
        info!(
            target: "partial_stateless",
            dir = %dir.display(),
            "Accessed-state fixture capture ENABLED (PS_CAPTURE_DIR) — run until ~300 blocks captured"
        );
    }

    // Optional benchmark-only comparison against the FULL witness (every accessed
    // key, ignoring the cache). It computes a second, larger multiproof per block,
    // so it is off by default and gated behind `PS_WITNESS_BASELINE`. Core sidecar
    // generation never depends on it.
    let compute_baseline = env_flag("PS_WITNESS_BASELINE");
    if compute_baseline {
        info!(
            target: "partial_stateless",
            "Full-witness baseline comparison ENABLED (PS_WITNESS_BASELINE) — extra multiproof per block"
        );
    }

    // Optional per-thread resource metrics (CPU time + page faults) captured
    // around the cold multiproof, to attribute its cost between compute and
    // disk I/O. Off by default and gated behind `PS_RESOURCE_METRICS`; when
    // disabled the getrusage syscalls are skipped and the metric fields stay None.
    let resource_metrics = env_flag("PS_RESOURCE_METRICS");
    if resource_metrics {
        info!(
            target: "partial_stateless",
            "Per-thread resource metrics ENABLED (PS_RESOURCE_METRICS) — cpu_time_ms + major/minor_page_faults per block"
        );
        #[cfg(not(target_os = "linux"))]
        warn!(
            target: "partial_stateless",
            "Per-thread CPU/page-fault metrics require Linux RUSAGE_THREAD; this platform will log default zeros for cpu_time_ms, major_page_faults, and minor_page_faults"
        );
        if compute_baseline {
            warn!(
                target: "partial_stateless",
                "PS_WITNESS_BASELINE runs before the partial multiproof; resource/page-fault metrics for the partial proof may be lower because the full baseline can warm the OS page cache"
            );
        }
    }

    // Optional provider-assisted validator preflight. This re-executes each
    // generated sidecar with a cache+witness-backed provider and checks the
    // cache-state transition. It is useful for PoC acceptance checks, but it
    // adds another block execution on the sidecar generation path.
    let run_sidecar_preflight = sidecar_role != SidecarRole::Verifier
        && (env_flag("PS_SIDECAR_PREFLIGHT") || sidecar_role == SidecarRole::BuilderVerifier);
    if run_sidecar_preflight {
        info!(
            target: "partial_stateless",
            sidecar_role = ?sidecar_role,
            "Provider-assisted sidecar preflight ENABLED — extra re-execution per sidecar"
        );
    }
    if sidecar_role == SidecarRole::Verifier {
        info!(
            target: "partial_stateless",
            sidecar_dir = %sidecar_dir.display(),
            verifier_wait_ms = verifier_wait.as_millis(),
            "Live sidecar verifier ENABLED — consuming sidecar files and advancing cache only after successful verification"
        );
    }
    let reexec_limits = SidecarReexecLimits::default();

    while let Some(notification) = ctx.notifications.try_next().await? {
        match &notification {
            ExExNotification::ChainCommitted { new } => {
                let range = new.range();
                let tip_block = *range.end();

                // Process blocks in chronological order
                for (block_number, block) in new.blocks() {
                    let parent_block_number = block_number.saturating_sub(1);

                    // Get state provider for the parent block to run execution simulation
                    let state_provider =
                        match ctx.provider().history_by_block_number(parent_block_number) {
                            Ok(provider) => provider,
                            Err(e) => {
                                warn!(
                                    target: "partial_stateless",
                                    block = *block_number,
                                    error = %e,
                                    "Failed to get state provider for parent block. Skipping block."
                                );
                                continue;
                            }
                        };

                    if sidecar_role == SidecarRole::Verifier {
                        if let Err(e) = verify_live_sidecar(
                            ctx.evm_config(),
                            state_provider.as_ref(),
                            block,
                            &mut cache,
                            &config,
                            &sidecar_dir,
                            &reexec_limits,
                            verifier_wait,
                        ) {
                            warn!(
                                target: "partial_stateless",
                                block = *block_number,
                                error = %e,
                                "Live sidecar verification failed; cache was not advanced"
                            );
                        }
                        continue;
                    }

                    let parent_state_root_by_hash = |parent_hash: B256| {
                        ctx.provider()
                            .sealed_header_by_hash(parent_hash)
                            .map_err(|err| eyre::eyre!("failed to fetch parent header: {err}"))?
                            .map(|header| header.state_root)
                            .ok_or_else(|| {
                                eyre::eyre!("parent header not found for hash {:?}", parent_hash)
                            })
                    };
                    let ancestor_headers_for_range =
                        |lowest_block_number: Option<u64>, block_number: u64| {
                            let Some(smallest) = lowest_block_number else {
                                return Ok(Vec::new());
                            };
                            let headers = ctx
                                .provider()
                                .headers_range(smallest..block_number)
                                .map_err(|err| {
                                    eyre::eyre!("failed to fetch ancestor headers for range: {err}")
                                })?;
                            Ok(headers
                                .into_iter()
                                .map(|header| {
                                    let mut buf = Vec::new();
                                    let _ = header.encode(&mut buf);
                                    buf.into()
                                })
                                .collect())
                        };

                    match create_sidecar_for_block(
                        ctx.evm_config(),
                        state_provider.as_ref(),
                        block,
                        &mut cache,
                        &config,
                        BuilderOptions {
                            capture_dir: capture_dir.as_deref(),
                            sidecar_dir: &sidecar_dir,
                            compute_baseline,
                            resource_metrics,
                            run_sidecar_preflight,
                            reexec_limits: &reexec_limits,
                        },
                        parent_state_root_by_hash,
                        ancestor_headers_for_range,
                    ) {
                        Ok(BuilderBlockReport {
                            cache_update: _cache_update,
                            witness: _witness,
                            sidecar_path: _sidecar_path,
                        }) => {}
                        Err(e) => {
                            warn!(
                                target: "partial_stateless",
                                block = *block_number,
                                error = %e,
                                "Sidecar builder block processing failed"
                            );
                        }
                    }
                }

                // Save updated cache state to file
                if let Err(e) = save_to_file(&cache, &cache_path) {
                    warn!(
                        target: "partial_stateless",
                        block = tip_block,
                        error = %e,
                        "Failed to save cache state to disk"
                    );
                }
            }
            ExExNotification::ChainReorged { old, new } => {
                let tip_block = *new.range().end();
                warn!(
                    target: "partial_stateless",
                    from_chain = ?old.range(),
                    to_chain = ?new.range(),
                    "Chain reorg detected — rolling back old blocks, then applying new chain"
                );

                // 1. Roll back the reverted (old) blocks newest→oldest, returning the
                //    cache to the fork point. On failure (history pruned/missing),
                //    cold-reset and let the new-chain loop below rebuild from scratch.
                let mut rollback_ok = true;
                for block_number in old.blocks().keys().rev() {
                    if let Err(e) = cache.rollback_block(*block_number) {
                        warn!(
                            target: "partial_stateless",
                            block = *block_number,
                            error = %e,
                            "Cache rollback failed on reorg — cold-resetting before reapply"
                        );
                        rollback_ok = false;
                        break;
                    }
                }
                if !rollback_ok {
                    cache.reset();
                }

                // 2. Apply the new canonical chain block-by-block (records undo).
                for (block_number, block) in new.blocks() {
                    let parent_block_number = block_number.saturating_sub(1);

                    let state_provider = match ctx
                        .provider()
                        .history_by_block_number(parent_block_number)
                    {
                        Ok(provider) => provider,
                        Err(e) => {
                            warn!(
                                target: "partial_stateless",
                                block = *block_number,
                                error = %e,
                                "Failed to get state provider for block parent on reorg. Skipping."
                            );
                            continue;
                        }
                    };

                    if sidecar_role == SidecarRole::Verifier {
                        if let Err(e) = verify_live_sidecar(
                            ctx.evm_config(),
                            state_provider.as_ref(),
                            block,
                            &mut cache,
                            &config,
                            &sidecar_dir,
                            &reexec_limits,
                            verifier_wait,
                        ) {
                            warn!(
                                target: "partial_stateless",
                                block = *block_number,
                                error = %e,
                                "Live sidecar verification failed while applying reorg; cache was not advanced"
                            );
                        }
                        continue;
                    }

                    let parent_state_root_by_hash = |parent_hash: B256| {
                        ctx.provider()
                            .sealed_header_by_hash(parent_hash)
                            .map_err(|err| eyre::eyre!("failed to fetch parent header: {err}"))?
                            .map(|header| header.state_root)
                            .ok_or_else(|| {
                                eyre::eyre!("parent header not found for hash {:?}", parent_hash)
                            })
                    };
                    let ancestor_headers_for_range =
                        |lowest_block_number: Option<u64>, block_number: u64| {
                            let Some(smallest) = lowest_block_number else {
                                return Ok(Vec::new());
                            };
                            let headers = ctx
                                .provider()
                                .headers_range(smallest..block_number)
                                .map_err(|err| {
                                    eyre::eyre!("failed to fetch ancestor headers for range: {err}")
                                })?;
                            Ok(headers
                                .into_iter()
                                .map(|header| {
                                    let mut buf = Vec::new();
                                    let _ = header.encode(&mut buf);
                                    buf.into()
                                })
                                .collect())
                        };

                    match create_sidecar_for_block(
                        ctx.evm_config(),
                        state_provider.as_ref(),
                        block,
                        &mut cache,
                        &config,
                        BuilderOptions {
                            capture_dir: capture_dir.as_deref(),
                            sidecar_dir: &sidecar_dir,
                            compute_baseline,
                            resource_metrics,
                            run_sidecar_preflight,
                            reexec_limits: &reexec_limits,
                        },
                        parent_state_root_by_hash,
                        ancestor_headers_for_range,
                    ) {
                        Ok(BuilderBlockReport {
                            cache_update: _cache_update,
                            witness: _witness,
                            sidecar_path: _sidecar_path,
                        }) => {}
                        Err(e) => {
                            warn!(
                                target: "partial_stateless",
                                block = *block_number,
                                error = %e,
                                "Sidecar builder block processing failed while applying reorg"
                            );
                        }
                    }
                }

                // Persist the rebuilt cache: a restart before the next commit must
                // not reload the stale pre-reorg (old-branch) cache from disk.
                if let Err(e) = save_to_file(&cache, &cache_path) {
                    warn!(
                        target: "partial_stateless",
                        block = tip_block,
                        error = %e,
                        "Failed to persist cache after reorg"
                    );
                }
            }
            ExExNotification::ChainReverted { old } => {
                warn!(
                    target: "partial_stateless",
                    reverted_chain = ?old.range(),
                    "Chain reverted — rolling back cache to the pre-revert state"
                );

                // Undo reverted blocks newest→oldest so each matches the top of the
                // undo stack. If a block can't be rolled back (history pruned/missing),
                // cold-reset: the cache rebuilds from subsequent canonical blocks.
                for block_number in old.blocks().keys().rev() {
                    if let Err(e) = cache.rollback_block(*block_number) {
                        warn!(
                            target: "partial_stateless",
                            block = *block_number,
                            error = %e,
                            "Cache rollback failed — cold-resetting cache"
                        );
                        cache.reset();
                        break;
                    }
                }

                // Persist the rolled-back cache: a restart before the next commit
                // must not reload the stale pre-revert cache from disk.
                if let Err(e) = save_to_file(&cache, &cache_path) {
                    warn!(
                        target: "partial_stateless",
                        error = %e,
                        "Failed to persist cache after revert"
                    );
                }
            }
        }

        // Prune undo history below the finalized block: reorgs never cross
        // finality, so once a block is finalized its undo record is unreachable.
        // Keeping records down to finality means any legal reorg can be rolled
        // back precisely (this cache has no re-execution fallback — a missing
        // undo record forces a cold reset). When finality is unavailable (early
        // sync / no-finality chains) fall back to a fixed depth floor so the log
        // stays bounded. Mirrors reth's CHANGESET_CACHE_RETENTION_BLOCKS.
        const UNDO_LOG_FALLBACK_DEPTH: u64 = 64;
        let threshold = ctx
            .provider()
            .finalized_block_number()
            .ok()
            .flatten()
            .unwrap_or_else(|| cache.current_block().saturating_sub(UNDO_LOG_FALLBACK_DEPTH));
        cache.prune_undo_below(threshold);

        // Acknowledge processed height
        if let Some(committed_chain) = notification.committed_chain() {
            ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().num_hash()))?;
        }
    }

    Ok(())
}

/// Format bytes into human-readable string.
fn format_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Per-thread resource snapshot used to attribute multiproof cost.
///
/// Returns `(cpu_micros, major_faults, minor_faults)` for the CALLING thread
/// only. We use `RUSAGE_THREAD` (not `RUSAGE_SELF`) so the numbers isolate the
/// synchronous `multiproof` call from the rest of the node's threads (execution,
/// networking, DB writes) running concurrently while the node follows tip.
///
/// Diagnostic use: comparing the CPU delta against the wall-clock elapsed
/// separates compute-bound blocks (cpu ≈ wall) from I/O/wait-bound blocks
/// (cpu ≪ wall); a nonzero major-fault delta proves the cold trie read hit
/// disk/swap rather than the page cache — the signature of the environmental
/// tail. Linux-only; returns zeros elsewhere or if the syscall fails.
#[cfg(target_os = "linux")]
fn thread_rusage() -> (u64, u64, u64) {
    // SAFETY: `getrusage` only writes into the `rusage` we hand it; the struct
    // is fully zero-initialized before the call.
    unsafe {
        let mut ru: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_THREAD, &mut ru) != 0 {
            return (0, 0, 0);
        }
        let cpu_us = (ru.ru_utime.tv_sec as u64) * 1_000_000
            + (ru.ru_utime.tv_usec as u64)
            + (ru.ru_stime.tv_sec as u64) * 1_000_000
            + (ru.ru_stime.tv_usec as u64);
        (cpu_us, ru.ru_majflt as u64, ru.ru_minflt as u64)
    }
}

#[cfg(not(target_os = "linux"))]
fn thread_rusage() -> (u64, u64, u64) {
    (0, 0, 0)
}

fn main() -> eyre::Result<()> {
    reth_ethereum::cli::Cli::parse_args().run(async move |builder, _| {
        let config = CacheConfig::default();

        let handle: NodeHandleFor<EthereumNode> = builder
            .node(EthereumNode::default())
            .install_exex("partial-stateless", move |ctx| async move {
                Ok(partial_stateless_exex(ctx, config))
            })
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}

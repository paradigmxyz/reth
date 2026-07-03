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

mod sidecar_reexec;

use alloy_primitives::Bytes;
use alloy_rlp::Encodable;
use futures::TryStreamExt;
use partial_stateless::{
    accessed_state::BlockAccessedState,
    fixture::{save_fixture, AccessedStateFixture},
    last_n_blocks_cache_policy_id,
    network_cache::NetworkStateCache,
    partial_witness_commitment,
    persistence::{load_from_file, save_to_file},
    policy::LastNBlocksPolicy,
    witness::{
        accessed_to_state_targets, build_sidecar_targets, cache_hit_targets,
        measure_multiproof_size, state_targets_to_proof_targets, WitnessResult,
    },
    CacheFootprintStats, PartialExecutionWitness, PartialExecutionWitnessState,
    PartialStatelessSidecar, RootWitnessCompletenessSummary, SerializableMultiProof,
    SidecarBenchmarkManifest, StateTargetSet, StateTargetStats, WitnessReductionStats,
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
    storage::StateProofProvider,
    EthPrimitives,
};
use reth_evm::{execute::Executor, ConfigureEvm};
use reth_provider::{BlockIdReader, HeaderProvider};
use reth_revm::database::StateProviderDatabase;
use reth_trie_common::TrieInput;
use revm::database::State;
use sidecar_reexec::{check_provider_assisted_sidecar, SidecarReexecLimits};
use std::fs;
use std::time::Instant;
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

    info!(
        target: "partial_stateless",
        account_window = config.account_window,
        storage_window = config.storage_window,
        cache_path = %cache_path.display(),
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
    let compute_baseline = std::env::var("PS_WITNESS_BASELINE")
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "on"))
        .unwrap_or(false);
    if compute_baseline {
        info!(
            target: "partial_stateless",
            "Full-witness baseline comparison ENABLED (PS_WITNESS_BASELINE) — extra multiproof per block"
        );
    }

    // Optional provider-assisted validator preflight. This re-executes each
    // generated sidecar with a cache+witness-backed provider and checks the
    // cache-state transition. It is useful for PoC acceptance checks, but it
    // adds another block execution on the sidecar generation path.
    let run_sidecar_preflight = std::env::var("PS_SIDECAR_PREFLIGHT")
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "on"))
        .unwrap_or(false);
    if run_sidecar_preflight {
        info!(
            target: "partial_stateless",
            "Provider-assisted sidecar preflight ENABLED (PS_SIDECAR_PREFLIGHT) — extra re-execution per sidecar"
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

                    let state_provider_db = StateProviderDatabase::new(&state_provider);
                    let mut db = State::builder()
                        .with_bundle_update()
                        .with_database(state_provider_db)
                        .build();

                    let block_executor = ctx.evm_config().executor(&mut db);

                    let mut accessed = BlockAccessedState::default();
                    let mut lowest_block_number = None;

                    let execution_output =
                        block_executor.execute_with_state_closure(block, |statedb: &State<_>| {
                            accessed = BlockAccessedState::from_simulated_state(statedb);
                            lowest_block_number = statedb.block_hashes.lowest().map(|(num, _)| num);
                        });

                    let execution_output = match execution_output {
                        Ok(output) => output,
                        Err(e) => {
                            warn!(
                                target: "partial_stateless",
                                block = *block_number,
                                error = %e,
                                "Simulation failed for block. Skipping block."
                            );
                            continue;
                        }
                    };

                    let hashed_post_state =
                        state_provider.hashed_post_state(&execution_output.state);
                    let computed_state_root = match state_provider
                        .state_root_with_updates(hashed_post_state)
                    {
                        Ok((root, _)) => root,
                        Err(e) => {
                            warn!(
                                target: "partial_stateless",
                                block = *block_number,
                                error = %e,
                                "Failed to compute post-execution state root. Skipping partial sidecar generation."
                            );
                            continue;
                        }
                    };
                    if computed_state_root != block.header().state_root {
                        warn!(
                            target: "partial_stateless",
                            block = *block_number,
                            expected_state_root = ?block.header().state_root,
                            computed_state_root = ?computed_state_root,
                            "Post-execution state root mismatch. Full witness fallback required."
                        );
                        continue;
                    }

                    // Capture the reproducible benchmark fixture (independent of the
                    // cache/sidecar pipeline below, so it survives later failures).
                    if let Some(dir) = &capture_dir {
                        let parent_state_root = ctx
                            .provider()
                            .sealed_header_by_hash(block.parent_hash)
                            .ok()
                            .flatten()
                            .map(|h| h.state_root)
                            .unwrap_or_default();
                        let fixture = AccessedStateFixture {
                            block_number: *block_number,
                            block_hash: block.hash(),
                            parent_state_root,
                            accessed: accessed.clone(),
                        };
                        match save_fixture(dir, &fixture) {
                            Ok(path) => info!(
                                target: "partial_stateless",
                                block = *block_number,
                                path = %path.display(),
                                accounts = accessed.accounts.len(),
                                storage = accessed.storage.len(),
                                codes = accessed.codes.len(),
                                "Captured accessed-state fixture"
                            ),
                            Err(e) => warn!(
                                target: "partial_stateless",
                                block = *block_number,
                                error = %e,
                                "Failed to capture accessed-state fixture"
                            ),
                        }
                    }

                    // Compute miss BEFORE updating cache (simulates what a validator would see)
                    let block_hash = block.hash();
                    let parent_hash = block.parent_hash;
                    let cache_policy_id =
                        last_n_blocks_cache_policy_id(config.account_window, config.storage_window);
                    let cache_policy_metadata = format!(
                        "LastNBlocks(account: {}, storage/code: {})",
                        config.account_window, config.storage_window
                    );
                    let cache_block_before = cache.current_block();
                    let cache_parent_synced = cache_block_before == parent_block_number;
                    if !cache_parent_synced {
                        warn!(
                            target: "partial_stateless",
                            block = *block_number,
                            cache_block = cache_block_before,
                            expected_parent_block = parent_block_number,
                            "Cache is not synced to the parent block. Metrics will update the local cache, but cache-coherent sidecar generation is disabled for this block."
                        );
                    }
                    let prev_cache_anchor = cache_parent_synced.then(|| {
                        cache.cache_anchor(parent_block_number, parent_hash, cache_policy_id)
                    });
                    let prev_cache_for_reexec = cache_parent_synced.then(|| {
                        NetworkStateCache::restore(
                            cache.accounts().clone(),
                            cache.storage().clone(),
                            cache.codes().clone(),
                            cache.current_block(),
                            Box::new(LastNBlocksPolicy::new(config.account_window)),
                            Box::new(LastNBlocksPolicy::new(config.storage_window)),
                        )
                    });
                    let cache_snapshot_before = cache.snapshot();
                    let cache_memory_before = cache.estimated_memory_bytes();
                    let miss = cache.compute_miss(&accessed);
                    let accessed_targets = accessed_to_state_targets(&accessed);
                    let cache_hit_targets = cache_hit_targets(&accessed, &miss);

                    // Now update the cache
                    let stats = cache.on_block_executed(*block_number, &accessed);
                    let next_cache_anchor = cache_parent_synced
                        .then(|| cache.cache_anchor(*block_number, block_hash, cache_policy_id));
                    let snapshot = cache.snapshot();
                    let cache_memory_after = cache.estimated_memory_bytes();

                    // Log comprehensive info
                    info!(
                        target: "partial_stateless",
                        block = *block_number,
                        "═══════════════════════════════════════════════════"
                    );
                    info!(
                        target: "partial_stateless",
                        block = *block_number,
                        accessed_accounts = accessed.accounts.len(),
                        accessed_storage = accessed.storage.len(),
                        accessed_codes = accessed.codes.len(),
                        total_accessed = accessed.total_keys(),
                        "Block state access"
                    );
                    info!(
                        target: "partial_stateless",
                        block = *block_number,
                        miss_ratio = format!("{:.1}%", miss.miss_ratio * 100.0),
                        missed_accounts = miss.missed_accounts.len(),
                        missed_storage = miss.missed_storage.len(),
                        missed_codes = miss.missed_codes.len(),
                        total_missed = miss.total_missed,
                        "Witness requirement (cache miss)"
                    );

                    // === Phase 2: Compute actual witness (Merkle proof) size & Generate Sidecar ===
                    let witness_result = {
                        // Extract raw targets and hashed multiproof targets in one pass
                        let (raw_targets, targets) = build_sidecar_targets(&miss);
                        let target_accounts = targets.len();
                        let target_slots: usize = targets.values().map(|slots| slots.len()).sum();

                        // Calculate total bytes of missed bytecodes
                        let missed_bytecode_bytes: usize = miss
                            .missed_codes
                            .iter()
                            .filter_map(|code_hash| accessed.codes.get(code_hash))
                            .map(|bytes| bytes.len())
                            .sum();

                        let missed_bytecodes: Vec<Bytes> = miss
                            .missed_codes
                            .iter()
                            .filter_map(|code_hash| accessed.codes.get(code_hash).cloned())
                            .collect();

                        // Optional full-witness baseline (every accessed key, ignoring the
                        // cache). Gated behind PS_WITNESS_BASELINE since it is an extra,
                        // larger multiproof. A failure here is non-fatal — it only drops the
                        // comparison for this block and never skips the real partial sidecar.
                        let full_sidecar_baseline_stats: Option<WitnessResult> = if compute_baseline
                        {
                            let full_targets = state_targets_to_proof_targets(&accessed_targets);
                            let full_target_accounts = full_targets.len();
                            let full_target_slots: usize =
                                full_targets.values().map(|slots| slots.len()).sum();
                            let full_bytecode_bytes: usize =
                                accessed.codes.values().map(|bytes| bytes.len()).sum();
                            let full_start = Instant::now();
                            match state_provider.multiproof(TrieInput::default(), full_targets) {
                                Ok(full_proof) => {
                                    let elapsed_ms = full_start.elapsed().as_millis() as u64;
                                    let mut full_result =
                                        measure_multiproof_size(&full_proof, full_bytecode_bytes);
                                    full_result.computation_time_ms = Some(elapsed_ms);
                                    full_result.target_accounts = full_target_accounts;
                                    full_result.target_storage_slots = full_target_slots;
                                    Some(full_result)
                                }
                                Err(e) => {
                                    warn!(
                                        target: "partial_stateless",
                                        block = *block_number,
                                        error = %e,
                                        "Failed to compute full sidecar baseline multiproof (comparison dropped for this block)"
                                    );
                                    None
                                }
                            }
                        } else {
                            None
                        };

                        let start = Instant::now();
                        // Compute multiproof with empty TrieInput (proof against DB state)
                        match state_provider.multiproof(TrieInput::default(), targets) {
                            Ok(proof) => {
                                let elapsed_ms = start.elapsed().as_millis() as u64;
                                let mut result =
                                    measure_multiproof_size(&proof, missed_bytecode_bytes);
                                result.computation_time_ms = Some(elapsed_ms);
                                result.target_accounts = target_accounts;
                                result.target_storage_slots = target_slots;

                                // --- Generate and Save Sidecar ---
                                let sidecar_generation_result = 'sidecar: {
                                    let (
                                        Some(prev_cache_anchor),
                                        Some(next_cache_anchor),
                                        Some(prev_cache_for_reexec),
                                    ) = (
                                        prev_cache_anchor,
                                        next_cache_anchor,
                                        prev_cache_for_reexec.as_ref(),
                                    )
                                    else {
                                        break 'sidecar Ok(());
                                    };

                                    let parent_header =
                                        match ctx.provider().sealed_header_by_hash(parent_hash) {
                                            Ok(Some(h)) => h,
                                            Ok(None) => {
                                                break 'sidecar Err(eyre::eyre!(
                                                    "Parent header not found for hash {:?}",
                                                    parent_hash
                                                ))
                                            }
                                            Err(e) => {
                                                break 'sidecar Err(eyre::eyre!(
                                                    "Failed to fetch parent header: {:?}",
                                                    e
                                                ))
                                            }
                                        };
                                    let parent_state_root = parent_header.state_root;

                                    // Check cache coherency
                                    if cache.current_block() != *block_number {
                                        warn!(
                                            target: "partial_stateless",
                                            block = *block_number,
                                            cache_block = cache.current_block(),
                                            expected_block = *block_number,
                                            "Cache state mismatch: cache block is not synced to block number. Skipping sidecar generation."
                                        );
                                        break 'sidecar Ok(());
                                    }

                                    // Fetch ancestor headers based on BLOCKHASH usage
                                    let ancestor_headers: Vec<Bytes> =
                                        if let Some(smallest) = lowest_block_number {
                                            let ancestor_range = smallest..*block_number;
                                            match ctx.provider().headers_range(ancestor_range) {
                                                Ok(headers) => headers
                                                    .into_iter()
                                                    .map(|header| {
                                                        let mut buf = Vec::new();
                                                        let _ = header.encode(&mut buf);
                                                        buf.into()
                                                    })
                                                    .collect(),
                                                Err(e) => {
                                                    break 'sidecar Err(eyre::eyre!(
                                                "Failed to fetch ancestor headers for range: {:?}",
                                                e
                                            ));
                                                }
                                            }
                                        } else {
                                            Vec::new()
                                        };

                                    // Convert proof to serializable format
                                    let serializable_proof =
                                        SerializableMultiProof::from_multiproof(&proof);
                                    let serialized_multiproof =
                                        match bincode::serialize(&serializable_proof) {
                                            Ok(b) => b,
                                            Err(e) => {
                                                break 'sidecar Err(eyre::eyre!(
                                                    "Failed to serialize multiproof: {:?}",
                                                    e
                                                ))
                                            }
                                        };

                                    let sidecar_miss = StateTargetSet::from(&raw_targets);
                                    let witness = PartialExecutionWitness {
                                        state: PartialExecutionWitnessState::MptMultiProof(
                                            serialized_multiproof,
                                        ),
                                        codes: missed_bytecodes.clone(),
                                        keys: raw_targets.key_preimages(),
                                        headers: ancestor_headers,
                                    };
                                    let witness_commitment = partial_witness_commitment(
                                        parent_state_root,
                                        &sidecar_miss,
                                        &witness,
                                    );

                                    let sidecar = PartialStatelessSidecar {
                                        parent_hash,
                                        parent_state_root,
                                        block_hash,
                                        block_number: *block_number,
                                        cache_block: parent_block_number,
                                        cache_policy_id,
                                        prev_cache_anchor,
                                        next_cache_anchor,
                                        cache_policy_metadata: cache_policy_metadata.clone(),
                                        cache_miss_targets: sidecar_miss.clone(),
                                        witness_commitment,
                                        miss_manifest: raw_targets.clone(),
                                        witness,
                                        stats: result.clone(),
                                    };

                                    let root_witness_completeness = if run_sidecar_preflight {
                                        let reexec_report = match check_provider_assisted_sidecar(
                                            ctx.evm_config(),
                                            state_provider.as_ref(),
                                            block,
                                            prev_cache_for_reexec,
                                            &sidecar,
                                            &config,
                                            &reexec_limits,
                                        ) {
                                            Ok(report) => report,
                                            Err(e) => {
                                                break 'sidecar Err(eyre::eyre!(
                                                    "Provider-assisted sidecar preflight failed: {e}"
                                                ));
                                            }
                                        };

                                        if !reexec_report
                                            .root_witness_completeness
                                            .trustless_root_ready
                                        {
                                            warn!(
                                                target: "partial_stateless",
                                                block = *block_number,
                                                missing_account_paths = reexec_report
                                                    .root_witness_completeness
                                                    .missing_account_paths
                                                    .len(),
                                                missing_storage_paths = reexec_report
                                                    .root_witness_completeness
                                                    .missing_storage_paths
                                                    .len(),
                                                "TODO: root witness incomplete for trustless state_root"
                                            );
                                        }
                                        info!(
                                            target: "partial_stateless",
                                            block = *block_number,
                                            computed_state_root = ?reexec_report.computed_state_root,
                                            reexec_accounts = reexec_report.actual_accessed.accounts.len(),
                                            reexec_storage = reexec_report.actual_accessed.storage.len(),
                                            reexec_codes = reexec_report.actual_accessed.codes.len(),
                                            expected_miss_accounts = reexec_report.expected_miss.accounts.len(),
                                            expected_miss_storage = reexec_report.expected_miss.storage.len(),
                                            expected_miss_codes = reexec_report.expected_miss.code_hashes.len(),
                                            next_cache_root = ?reexec_report.next_cache_anchor.cache_root,
                                            "Provider-assisted sidecar preflight succeeded"
                                        );
                                        RootWitnessCompletenessSummary::from_report(
                                            &reexec_report.root_witness_completeness,
                                        )
                                    } else {
                                        RootWitnessCompletenessSummary::default()
                                    };

                                    // Ensure sidecar directory exists under workspace root
                                    let sidecar_dir = std::env::current_dir()
                                        .unwrap_or_else(|_| std::path::PathBuf::from("."))
                                        .join("sidecar");
                                    if let Err(e) = fs::create_dir_all(&sidecar_dir) {
                                        break 'sidecar Err(eyre::eyre!(
                                            "Failed to create sidecar directory: {:?}",
                                            e
                                        ));
                                    }

                                    let sidecar_filename =
                                        format!("block_{}_{:?}.bin", block_number, block.hash());
                                    let sidecar_path = sidecar_dir.join(sidecar_filename);

                                    let sidecar_bytes = match bincode::serialize(&sidecar) {
                                        Ok(b) => b,
                                        Err(e) => {
                                            break 'sidecar Err(eyre::eyre!(
                                                "Failed to serialize sidecar: {:?}",
                                                e
                                            ))
                                        }
                                    };

                                    if let Err(e) = fs::write(&sidecar_path, sidecar_bytes) {
                                        break 'sidecar Err(eyre::eyre!(
                                            "Failed to write sidecar file {:?}: {:?}",
                                            sidecar_path,
                                            e
                                        ));
                                    }

                                    let sidecar_bytes_len = fs::metadata(&sidecar_path)
                                        .map(|m| m.len() as usize)
                                        .unwrap_or(0);
                                    let manifest = SidecarBenchmarkManifest {
                                        schema_version: 6,
                                        block_number: *block_number,
                                        block_hash,
                                        parent_hash,
                                        parent_state_root,
                                        cache_block: parent_block_number,
                                        cache_policy_id,
                                        prev_cache_anchor,
                                        next_cache_anchor,
                                        cache_policy_metadata: cache_policy_metadata.clone(),
                                        sidecar_file: sidecar_path
                                            .file_name()
                                            .map(|name| name.to_string_lossy().into_owned())
                                            .unwrap_or_else(|| sidecar_path.display().to_string()),
                                        sidecar_bytes: sidecar_bytes_len,
                                        cache_before: CacheFootprintStats::new(
                                            cache_snapshot_before.total_accounts,
                                            cache_snapshot_before.total_storage_slots,
                                            cache_snapshot_before.total_codes,
                                            cache_memory_before,
                                        ),
                                        cache_after: CacheFootprintStats::new(
                                            snapshot.total_accounts,
                                            snapshot.total_storage_slots,
                                            snapshot.total_codes,
                                            cache_memory_after,
                                        ),
                                        accessed: StateTargetStats::from_targets(&accessed_targets),
                                        cache_hit: StateTargetStats::from_targets(
                                            &cache_hit_targets,
                                        ),
                                        sidecar_miss: StateTargetStats::from_targets(&sidecar_miss),
                                        provider_assisted_preflight: run_sidecar_preflight,
                                        root_witness_completeness,
                                        full_sidecar_baseline_stats: full_sidecar_baseline_stats
                                            .clone(),
                                        partial_sidecar_stats: result.clone(),
                                        reduction: full_sidecar_baseline_stats
                                            .as_ref()
                                            .map(|full| WitnessReductionStats::new(&result, full)),
                                    };
                                    let manifest_path =
                                        sidecar_path.with_extension("manifest.json");
                                    let manifest_bytes = match serde_json::to_vec_pretty(&manifest)
                                    {
                                        Ok(bytes) => bytes,
                                        Err(e) => {
                                            break 'sidecar Err(eyre::eyre!(
                                                "Failed to serialize sidecar manifest: {:?}",
                                                e
                                            ))
                                        }
                                    };
                                    if let Err(e) = fs::write(&manifest_path, manifest_bytes) {
                                        break 'sidecar Err(eyre::eyre!(
                                            "Failed to write sidecar manifest {:?}: {:?}",
                                            manifest_path,
                                            e
                                        ));
                                    }

                                    info!(
                                        target: "partial_stateless",
                                        block = *block_number,
                                        path = %sidecar_path.display(),
                                        manifest = %manifest_path.display(),
                                        size = format_bytes(sidecar_bytes_len),
                                        "Saved witness sidecar successfully"
                                    );

                                    Ok(())
                                };

                                if let Err(e) = sidecar_generation_result {
                                    warn!(
                                        target: "partial_stateless",
                                        block = *block_number,
                                        error = %e,
                                        "Sidecar generation failed (non-fatal)"
                                    );
                                }

                                Some(result)
                            }
                            Err(e) => {
                                warn!(
                                    target: "partial_stateless",
                                    block = *block_number,
                                    error = %e,
                                    "Failed to compute multiproof"
                                );
                                None
                            }
                        }
                    };

                    if let Some(witness) = witness_result {
                        info!(
                            target: "partial_stateless",
                            block = *block_number,
                            witness_total_bytes = witness.total_size_bytes,
                            witness_total = format_bytes(witness.total_size_bytes),
                            account_proof_bytes = witness.account_proof_bytes,
                            account_proof = format_bytes(witness.account_proof_bytes),
                            storage_proof_bytes = witness.storage_proof_bytes,
                            storage_proof = format_bytes(witness.storage_proof_bytes),
                            bytecode_bytes = witness.bytecode_bytes,
                            bytecode_size = format_bytes(witness.bytecode_bytes),
                            account_proof_nodes = witness.account_proof_nodes,
                            storage_proof_nodes = witness.storage_proof_nodes,
                            target_accounts = witness.target_accounts,
                            target_storage_slots = witness.target_storage_slots,
                            computation_time_ms = witness.computation_time_ms.unwrap_or(0),
                            "Witness size (Merkle proof)"
                        );
                    }

                    info!(
                        target: "partial_stateless",
                        block = *block_number,
                        cache_accounts = snapshot.total_accounts,
                        cache_storage = snapshot.total_storage_slots,
                        cache_codes = snapshot.total_codes,
                        estimated_memory = format_bytes(cache.estimated_memory_bytes()),
                        accounts_added = stats.accounts_added,
                        accounts_refreshed = stats.accounts_refreshed,
                        accounts_evicted = stats.accounts_evicted,
                        storage_added = stats.storage_added,
                        storage_refreshed = stats.storage_refreshed,
                        storage_evicted = stats.storage_evicted,
                        "Cache state after update"
                    );
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

                    let state_provider_db = StateProviderDatabase::new(&state_provider);
                    let mut db = State::builder()
                        .with_bundle_update()
                        .with_database(state_provider_db)
                        .build();

                    let block_executor = ctx.evm_config().executor(&mut db);

                    let mut accessed = BlockAccessedState::default();
                    let sim_result =
                        block_executor.execute_with_state_closure(block, |statedb: &State<_>| {
                            accessed = BlockAccessedState::from_simulated_state(statedb);
                        });

                    if let Err(e) = sim_result {
                        warn!(
                            target: "partial_stateless",
                            block = *block_number,
                            error = %e,
                            "Simulation failed on reorg. Skipping."
                        );
                        continue;
                    }

                    cache.on_block_executed(*block_number, &accessed);
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

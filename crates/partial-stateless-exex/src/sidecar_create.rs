use crate::{
    format_bytes,
    sidecar_io::sidecar_path,
    sidecar_reexec::{verify_and_apply_provider_assisted_sidecar, SidecarReexecLimits},
    thread_rusage, CacheConfig,
};
use alloy_primitives::{Bytes, B256};
use partial_stateless::{
    accessed_state::BlockAccessedState,
    fixture::{save_fixture, AccessedStateFixture},
    last_n_blocks_cache_policy_id,
    network_cache::{NetworkStateCache, UpdateStats},
    partial_witness_commitment,
    policy::LastNBlocksPolicy,
    witness::{
        accessed_to_state_targets, build_sidecar_targets, cache_hit_targets,
        measure_multiproof_size, state_targets_to_proof_targets, WitnessResult,
    },
    CacheFootprintStats, PartialExecutionWitness, PartialExecutionWitnessState,
    PartialStatelessSidecar, RootWitnessCompletenessSummary, SerializableMultiProof,
    SidecarBenchmarkManifest, StateTargetSet, StateTargetStats, WitnessReductionStats,
};
use reth_ethereum::EthPrimitives;
use reth_evm::{execute::Executor, ConfigureEvm};
use reth_primitives_traits::{AlloyBlockHeader, BlockTy, RecoveredBlock};
use reth_provider::StateProvider;
use reth_revm::database::StateProviderDatabase;
use reth_trie_common::TrieInput;
use revm::database::State;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Instant,
};
use tracing::{info, warn};

pub(crate) struct BuilderOptions<'a> {
    pub(crate) capture_dir: Option<&'a Path>,
    pub(crate) sidecar_dir: &'a Path,
    pub(crate) compute_baseline: bool,
    pub(crate) resource_metrics: bool,
    pub(crate) run_sidecar_preflight: bool,
    pub(crate) reexec_limits: &'a SidecarReexecLimits,
}

#[derive(Debug)]
pub(crate) struct BuilderBlockReport {
    pub(crate) cache_update: UpdateStats,
    pub(crate) witness: Option<WitnessResult>,
    pub(crate) sidecar_path: Option<PathBuf>,
}

fn rollback_sidecar_transition(
    cache: &mut NetworkStateCache,
    block_number: u64,
    cause: eyre::Report,
) -> eyre::Report {
    match cache.rollback_block(block_number) {
        Ok(()) => eyre::eyre!(
            "sidecar generation failed; cache transition for block {block_number} was rolled back: {cause:#}"
        ),
        Err(rollback_err) => eyre::eyre!(
            "sidecar generation failed for block {block_number} ({cause:#}); cache rollback also failed: {rollback_err}"
        ),
    }
}

pub(crate) fn create_sidecar_for_block<Evm, ParentStateRootFn, AncestorHeadersFn>(
    evm_config: &Evm,
    state_provider: &dyn StateProvider,
    block: &RecoveredBlock<BlockTy<EthPrimitives>>,
    cache: &mut NetworkStateCache,
    config: &CacheConfig,
    options: BuilderOptions<'_>,
    parent_state_root_by_hash: ParentStateRootFn,
    ancestor_headers_for_range: AncestorHeadersFn,
) -> eyre::Result<BuilderBlockReport>
where
    Evm: ConfigureEvm<Primitives = EthPrimitives>,
    ParentStateRootFn: FnOnce(B256) -> eyre::Result<B256>,
    AncestorHeadersFn: FnOnce(Option<u64>, u64) -> eyre::Result<Vec<Bytes>>,
{
    let block_number = block.number();
    let parent_block_number = block_number.saturating_sub(1);

    let state_provider_db = StateProviderDatabase::new(state_provider);
    let mut db = State::builder().with_bundle_update().with_database(state_provider_db).build();
    let block_executor = evm_config.executor(&mut db);

    let mut accessed = BlockAccessedState::default();
    let mut lowest_block_number = None;
    let execution_output = block_executor
        .execute_with_state_closure(block, |statedb: &State<_>| {
            accessed = BlockAccessedState::from_simulated_state(statedb);
            lowest_block_number = statedb.block_hashes.lowest().map(|(num, _)| num);
        })
        .map_err(|err| eyre::eyre!("simulation failed for block: {err}"))?;

    let hashed_post_state = state_provider.hashed_post_state(&execution_output.state);
    let (computed_state_root, _) = state_provider
        .state_root_with_updates(hashed_post_state)
        .map_err(|err| eyre::eyre!("failed to compute post-execution state root: {err}"))?;
    if computed_state_root != block.header().state_root {
        return Err(eyre::eyre!(
            "post-execution state root mismatch: expected {:?}, got {:?}",
            block.header().state_root,
            computed_state_root
        ));
    }

    let block_hash = block.hash();
    let parent_hash = block.parent_hash;
    let parent_state_root_result = parent_state_root_by_hash(parent_hash);

    if let Some(dir) = options.capture_dir {
        let fixture = AccessedStateFixture {
            block_number,
            block_hash,
            parent_state_root: parent_state_root_result.as_ref().copied().unwrap_or_default(),
            accessed: accessed.clone(),
        };
        match save_fixture(dir, &fixture) {
            Ok(path) => info!(
                target: "partial_stateless",
                block = block_number,
                path = %path.display(),
                accounts = accessed.accounts.len(),
                storage = accessed.storage.len(),
                codes = accessed.codes.len(),
                "Captured accessed-state fixture"
            ),
            Err(e) => warn!(
                target: "partial_stateless",
                block = block_number,
                error = %e,
                "Failed to capture accessed-state fixture"
            ),
        }
    }

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
            block = block_number,
            cache_block = cache_block_before,
            expected_parent_block = parent_block_number,
            "Cache is not synced to the parent block. Metrics will update the local cache, but cache-coherent sidecar generation is disabled for this block."
        );
    }
    let prev_cache_anchor = cache_parent_synced
        .then(|| cache.cache_anchor(parent_block_number, parent_hash, cache_policy_id));
    let mut prev_cache_for_reexec = cache_parent_synced.then(|| {
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

    let stats = cache.on_block_executed(block_number, &accessed);
    let next_cache_anchor =
        cache_parent_synced.then(|| cache.cache_anchor(block_number, block_hash, cache_policy_id));
    let snapshot = cache.snapshot();
    let cache_memory_after = cache.estimated_memory_bytes();

    info!(
        target: "partial_stateless",
        block = block_number,
        "═══════════════════════════════════════════════════"
    );
    info!(
        target: "partial_stateless",
        block = block_number,
        accessed_accounts = accessed.accounts.len(),
        accessed_storage = accessed.storage.len(),
        accessed_codes = accessed.codes.len(),
        total_accessed = accessed.total_keys(),
        "Block state access"
    );
    info!(
        target: "partial_stateless",
        block = block_number,
        miss_ratio = format!("{:.1}%", miss.miss_ratio * 100.0),
        missed_accounts = miss.missed_accounts.len(),
        missed_storage = miss.missed_storage.len(),
        missed_codes = miss.missed_codes.len(),
        total_missed = miss.total_missed,
        "Witness requirement (cache miss)"
    );

    let (raw_targets, targets) = build_sidecar_targets(&miss);
    let target_accounts = targets.len();
    let target_slots: usize = targets.values().map(|slots| slots.len()).sum();
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

    let full_sidecar_baseline_stats: Option<WitnessResult> = if options.compute_baseline {
        let full_targets = state_targets_to_proof_targets(&accessed_targets);
        let full_target_accounts = full_targets.len();
        let full_target_slots: usize = full_targets.values().map(|slots| slots.len()).sum();
        let full_bytecode_bytes: usize = accessed.codes.values().map(|bytes| bytes.len()).sum();
        let full_start = Instant::now();
        match state_provider.multiproof(TrieInput::default(), full_targets) {
            Ok(full_proof) => {
                let elapsed_ms = full_start.elapsed().as_millis() as u64;
                let mut full_result = measure_multiproof_size(&full_proof, full_bytecode_bytes);
                full_result.computation_time_ms = Some(elapsed_ms);
                full_result.target_accounts = full_target_accounts;
                full_result.target_storage_slots = full_target_slots;
                Some(full_result)
            }
            Err(e) => {
                warn!(
                    target: "partial_stateless",
                    block = block_number,
                    error = %e,
                    "Failed to compute full sidecar baseline multiproof (comparison dropped for this block)"
                );
                None
            }
        }
    } else {
        None
    };

    let rusage_before = options.resource_metrics.then(thread_rusage);
    let start = Instant::now();
    let mut saved_sidecar_path = None;
    let witness = match state_provider.multiproof(TrieInput::default(), targets) {
        Ok(proof) => {
            let elapsed_ms = start.elapsed().as_millis() as u64;
            let mut result = measure_multiproof_size(&proof, missed_bytecode_bytes);
            result.computation_time_ms = Some(elapsed_ms);
            if let Some((cpu_us_before, majflt_before, minflt_before)) = rusage_before {
                let (cpu_us_after, majflt_after, minflt_after) = thread_rusage();
                result.cpu_time_ms = Some(cpu_us_after.saturating_sub(cpu_us_before) / 1000);
                result.major_page_faults = Some(majflt_after.saturating_sub(majflt_before));
                result.minor_page_faults = Some(minflt_after.saturating_sub(minflt_before));
            }
            result.target_accounts = target_accounts;
            result.target_storage_slots = target_slots;

            let sidecar_generation_result: eyre::Result<Option<PathBuf>> = 'sidecar: {
                let (Some(prev_cache_anchor), Some(next_cache_anchor), Some(prev_cache_for_reexec)) =
                    (prev_cache_anchor, next_cache_anchor, prev_cache_for_reexec.as_mut())
                else {
                    break 'sidecar Ok(None);
                };
                let parent_state_root = parent_state_root_result?;

                if cache.current_block() != block_number {
                    warn!(
                        target: "partial_stateless",
                        block = block_number,
                        cache_block = cache.current_block(),
                        expected_block = block_number,
                        "Cache state mismatch: cache block is not synced to block number. Skipping sidecar generation."
                    );
                    break 'sidecar Ok(None);
                }

                let ancestor_headers =
                    ancestor_headers_for_range(lowest_block_number, block_number)?;
                let serializable_proof = SerializableMultiProof::from_multiproof(&proof);
                let serialized_multiproof = bincode::serialize(&serializable_proof)
                    .map_err(|err| eyre::eyre!("failed to serialize multiproof: {err}"))?;
                let sidecar_miss = StateTargetSet::from(&raw_targets);
                let witness_payload = PartialExecutionWitness {
                    state: PartialExecutionWitnessState::MptMultiProof(serialized_multiproof),
                    codes: missed_bytecodes.clone(),
                    keys: raw_targets.key_preimages(),
                    headers: ancestor_headers,
                };
                let witness_commitment =
                    partial_witness_commitment(parent_state_root, &sidecar_miss, &witness_payload);
                let sidecar = PartialStatelessSidecar {
                    parent_hash,
                    parent_state_root,
                    block_hash,
                    block_number,
                    cache_block: parent_block_number,
                    cache_policy_id,
                    prev_cache_anchor,
                    next_cache_anchor,
                    cache_policy_metadata: cache_policy_metadata.clone(),
                    cache_miss_targets: sidecar_miss.clone(),
                    witness_commitment,
                    miss_manifest: raw_targets.clone(),
                    witness: witness_payload,
                    stats: result.clone(),
                };

                let root_witness_completeness = if options.run_sidecar_preflight {
                    let reexec_report = verify_and_apply_provider_assisted_sidecar(
                        evm_config,
                        state_provider,
                        block,
                        prev_cache_for_reexec,
                        &sidecar,
                        options.reexec_limits,
                    )
                    .map_err(|err| {
                        eyre::eyre!("provider-assisted sidecar preflight failed: {err}")
                    })?;

                    if !reexec_report.root_witness_completeness.trustless_root_ready {
                        warn!(
                            target: "partial_stateless",
                            block = block_number,
                            partial_state_trustless_verification_ready = false,
                            missing_account_paths = reexec_report
                                .root_witness_completeness
                                .missing_account_paths
                                .len(),
                            missing_storage_paths = reexec_report
                                .root_witness_completeness
                                .missing_storage_paths
                                .len(),
                            "Partial-state node trustless verification is not ready; current state_root check is provider-assisted"
                        );
                    }
                    info!(
                        target: "partial_stateless",
                        block = block_number,
                        partial_state_trustless_verification_ready = reexec_report
                            .root_witness_completeness
                            .trustless_root_ready,
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

                fs::create_dir_all(options.sidecar_dir)
                    .map_err(|err| eyre::eyre!("failed to create sidecar directory: {err}"))?;
                let sidecar_path = sidecar_path(options.sidecar_dir, block_number, block_hash);
                let sidecar_bytes = bincode::serialize(&sidecar)
                    .map_err(|err| eyre::eyre!("failed to serialize sidecar: {err}"))?;
                fs::write(&sidecar_path, sidecar_bytes).map_err(|err| {
                    eyre::eyre!("failed to write sidecar file {:?}: {err}", sidecar_path)
                })?;
                let sidecar_bytes_len =
                    fs::metadata(&sidecar_path).map(|m| m.len() as usize).unwrap_or(0);
                let partial_state_trustless_verification_ready =
                    root_witness_completeness.trustless_root_ready;
                let manifest = SidecarBenchmarkManifest {
                    schema_version: 1,
                    block_number,
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
                    cache_hit: StateTargetStats::from_targets(&cache_hit_targets),
                    sidecar_miss: StateTargetStats::from_targets(&sidecar_miss),
                    provider_assisted_preflight: options.run_sidecar_preflight,
                    partial_state_trustless_verification_ready,
                    root_witness_completeness,
                    full_sidecar_baseline_stats: full_sidecar_baseline_stats.clone(),
                    partial_sidecar_stats: result.clone(),
                    reduction: full_sidecar_baseline_stats
                        .as_ref()
                        .map(|full| WitnessReductionStats::new(&result, full)),
                };
                let manifest_path = sidecar_path.with_extension("manifest.json");
                let manifest_saved = match serde_json::to_vec_pretty(&manifest) {
                    Ok(manifest_bytes) => match fs::write(&manifest_path, manifest_bytes) {
                        Ok(()) => true,
                        Err(err) => {
                            warn!(
                                target: "partial_stateless",
                                block = block_number,
                                path = %manifest_path.display(),
                                error = %err,
                                "Failed to write diagnostic sidecar manifest"
                            );
                            false
                        }
                    },
                    Err(err) => {
                        warn!(
                            target: "partial_stateless",
                            block = block_number,
                            error = %err,
                            "Failed to serialize diagnostic sidecar manifest"
                        );
                        false
                    }
                };
                info!(
                    target: "partial_stateless",
                    block = block_number,
                    path = %sidecar_path.display(),
                    manifest = %manifest_path.display(),
                    manifest_saved,
                    size = format_bytes(sidecar_bytes_len),
                    "Saved witness sidecar successfully"
                );
                Ok(Some(sidecar_path))
            };

            match sidecar_generation_result {
                Ok(path) => saved_sidecar_path = path,
                Err(e) if cache_parent_synced => {
                    return Err(rollback_sidecar_transition(cache, block_number, e));
                }
                Err(e) => warn!(
                    target: "partial_stateless",
                    block = block_number,
                    error = %e,
                    "Sidecar generation failed while cache was not coherent"
                ),
            }
            Some(result)
        }
        Err(e) => {
            if cache_parent_synced {
                return Err(rollback_sidecar_transition(
                    cache,
                    block_number,
                    eyre::eyre!("failed to compute sidecar multiproof: {e}"),
                ));
            }
            warn!(
                target: "partial_stateless",
                block = block_number,
                error = %e,
                "Failed to compute multiproof while cache was not coherent"
            );
            None
        }
    };

    if let Some(witness) = &witness {
        info!(
            target: "partial_stateless",
            block = block_number,
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
            cpu_time_ms = witness.cpu_time_ms.unwrap_or(0),
            major_page_faults = witness.major_page_faults.unwrap_or(0),
            minor_page_faults = witness.minor_page_faults.unwrap_or(0),
            "Witness size (Merkle proof)"
        );
    }

    info!(
        target: "partial_stateless",
        block = block_number,
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

    Ok(BuilderBlockReport { cache_update: stats, witness, sidecar_path: saved_sidecar_path })
}

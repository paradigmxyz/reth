use crate::{
    sidecar_io::{read_sidecar, sidecar_path},
    sidecar_reexec::{verify_and_apply_provider_assisted_sidecar, SidecarReexecLimits},
    CacheConfig,
};
use partial_stateless::{
    last_n_blocks_cache_policy_id, network_cache::NetworkStateCache, PartialTrieNodeCache,
};
use reth_ethereum::EthPrimitives;
use reth_evm::ConfigureEvm;
use reth_primitives_traits::{AlloyBlockHeader, BlockTy, RecoveredBlock};
use reth_provider::StateProvider;
use std::{path::Path, time::Duration};
use tracing::{info, warn};

pub(crate) fn verify_live_sidecar<Evm>(
    evm_config: &Evm,
    state_provider: &dyn StateProvider,
    block: &RecoveredBlock<BlockTy<EthPrimitives>>,
    cache: &mut NetworkStateCache,
    trie_cache: &mut PartialTrieNodeCache,
    config: &CacheConfig,
    sidecar_dir: &Path,
    limits: &SidecarReexecLimits,
    wait: Duration,
) -> eyre::Result<()>
where
    Evm: ConfigureEvm<Primitives = EthPrimitives>,
{
    let block_number = block.number();
    let block_hash = block.hash();
    let parent_block_number = block_number.saturating_sub(1);
    if cache.current_block() != parent_block_number {
        return Err(eyre::eyre!(
            "verifier cache is not synced to parent: cache_block={}, expected_parent={}",
            cache.current_block(),
            parent_block_number
        ));
    }

    let path = sidecar_path(sidecar_dir, block_number, block_hash);
    let (bytes, sidecar) = read_sidecar(&path, wait)?;

    let expected_policy_id =
        last_n_blocks_cache_policy_id(config.account_window, config.storage_window);
    if sidecar.cache_policy_id != expected_policy_id {
        return Err(eyre::eyre!(
            "sidecar cache_policy_id mismatch: expected {:?}, got {:?}",
            expected_policy_id,
            sidecar.cache_policy_id
        ));
    }

    let report = verify_and_apply_provider_assisted_sidecar(
        evm_config,
        state_provider,
        block,
        cache,
        &sidecar,
        limits,
        trie_cache,
    )?;

    if !report.root_witness_completeness.trustless_root_ready {
        warn!(
            target: "partial_stateless",
            block = block_number,
            partial_state_trustless_verification_ready = false,
            missing_account_paths = report.root_witness_completeness.missing_account_paths.len(),
            missing_storage_paths = report.root_witness_completeness.missing_storage_paths.len(),
            "Partial-state node trustless verification is not ready; current state_root check is provider-assisted"
        );
    }

    match report.trustless_state_root {
        Some(root) if root == block.state_root() => info!(
            target: "partial_stateless",
            block = block_number,
            trustless_state_root = ?root,
            "Trustless state root VERIFIED (trie node cache + witness only)"
        ),
        Some(root) => warn!(
            target: "partial_stateless",
            block = block_number,
            trustless_state_root = ?root,
            expected = ?block.state_root(),
            "Trustless state root MISMATCH — trie cache/witness stale or insufficient"
        ),
        None => info!(
            target: "partial_stateless",
            block = block_number,
            trie_warm_nodes = trie_cache.warm_node_count(),
            tracked_accounts = trie_cache.tracked_account_count(),
            "Trustless state root unavailable — trie node cache not warm enough this block (blind path)"
        ),
    }

    let stats = &report.cache_update;
    info!(
        target: "partial_stateless",
        block = block_number,
        path = %path.display(),
        sidecar_bytes = bytes.len(),
        partial_state_trustless_verification_ready = report
            .root_witness_completeness
            .trustless_root_ready,
        computed_state_root = ?report.computed_state_root,
        reexec_accounts = report.actual_accessed.accounts.len(),
        reexec_storage = report.actual_accessed.storage.len(),
        reexec_codes = report.actual_accessed.codes.len(),
        expected_miss_accounts = report.expected_miss.accounts.len(),
        expected_miss_storage = report.expected_miss.storage.len(),
        expected_miss_codes = report.expected_miss.code_hashes.len(),
        next_cache_root = ?report.next_cache_anchor.cache_root,
        accounts_added = stats.accounts_added,
        accounts_refreshed = stats.accounts_refreshed,
        accounts_evicted = stats.accounts_evicted,
        storage_added = stats.storage_added,
        storage_refreshed = stats.storage_refreshed,
        storage_evicted = stats.storage_evicted,
        "Live sidecar verification succeeded"
    );

    Ok(())
}

use alloy_primitives::{Address, Bytes, B256, U256};
use eyre::{bail, eyre, Result};
use partial_stateless::{
    accessed_state::BlockAccessedState,
    check_sidecar_context, check_sidecar_miss_targets, compute_trustless_state_root,
    network_cache::{NetworkStateCache, UpdateStats},
    witness_check::{
        materialize_sidecar_witness_with_limits, root_witness_completeness_from_bundle,
        SidecarWitnessCheckLimits,
    },
    CacheAnchor, PartialStatelessSidecar, PartialTrieNodeCache, RootWitnessCompletenessReport,
    StateTargetSet,
};
use reth_ethereum::EthPrimitives;
use reth_evm::{execute::Executor, ConfigureEvm};
use reth_primitives_traits::{Account, AlloyBlockHeader, BlockTy, Bytecode, RecoveredBlock};
use reth_provider::{ProviderError, ProviderResult, StateProvider};
use reth_revm::database::{EvmStateProvider, StateProviderDatabase};
use revm::database::State;
use std::collections::HashMap;

pub(crate) type SidecarReexecLimits = SidecarWitnessCheckLimits;

#[derive(Debug, Clone)]
pub(crate) struct SidecarReexecReport {
    pub computed_state_root: B256,
    pub actual_accessed: BlockAccessedState,
    pub expected_miss: StateTargetSet,
    pub next_cache_anchor: CacheAnchor,
    pub cache_update: UpdateStats,
    pub root_witness_completeness: RootWitnessCompletenessReport,
    /// State root computed trustlessly from the trie-node cache + witness only (no full-DB trie
    /// access). `None` when the cache was not warm enough (a cache-hit path is blind).
    pub trustless_state_root: Option<B256>,
}

pub(crate) fn verify_and_apply_provider_assisted_sidecar<Evm>(
    evm_config: &Evm,
    full_provider: &dyn StateProvider,
    block: &RecoveredBlock<BlockTy<EthPrimitives>>,
    prev_cache: &mut NetworkStateCache,
    sidecar: &PartialStatelessSidecar,
    limits: &SidecarReexecLimits,
    trie_cache: &mut PartialTrieNodeCache,
) -> Result<SidecarReexecReport>
where
    Evm: ConfigureEvm<Primitives = EthPrimitives>,
{
    prefilter(block, prev_cache, sidecar)?;

    let materialized = materialize_sidecar_witness_with_limits(sidecar, limits, trie_cache)
        .map_err(|err| eyre!("sidecar witness check failed: {err}"))?;
    // Retained for trustless root computation before the other fields are moved into the provider.
    let witness_multiproof = materialized.multiproof;
    let witness_provider = WitnessBackedStateProvider {
        cache: prev_cache,
        witness_accounts: materialized.accounts,
        witness_storage: materialized.storage,
        witness_codes: materialized.codes,
        witness_headers: materialized.headers,
        block_number: sidecar.block_number,
    };

    let state_provider_db = StateProviderDatabase::new(witness_provider);
    let mut db = State::builder().with_bundle_update().with_database(state_provider_db).build();
    let block_executor = evm_config.executor(&mut db);

    let mut actual_accessed = BlockAccessedState::default();
    let execution_output = block_executor
        .execute_with_state_closure(block, |statedb: &State<_>| {
            actual_accessed = BlockAccessedState::from_simulated_state(statedb);
        })
        .map_err(|err| eyre!("partial sidecar re-execution failed: {err:?}"))?;
    drop(db);

    // Trustless post-state root: trie-node cache + witness only, no full-DB trie access. This is
    // the stateless-validator path. `None` means the cache was not warm enough this block; the
    // provider-assisted root below stays the ground-truth cross-check.
    let trustless_state_root = compute_trustless_state_root(
        witness_multiproof.clone(),
        trie_cache,
        &execution_output.state,
    );

    let hashed_post_state = full_provider.hashed_post_state(&execution_output.state);
    let (computed_state_root, _) = full_provider
        .state_root_with_updates(hashed_post_state)
        .map_err(|err| eyre!("provider-assisted state root failed: {err}"))?;
    if computed_state_root != block.state_root() {
        bail!(
            "provider-assisted state root mismatch: expected {:?}, got {:?}",
            block.state_root(),
            computed_state_root
        );
    }

    let expected_miss = prev_cache.expected_miss_targets(&actual_accessed);
    check_sidecar_miss_targets(sidecar, &expected_miss)
        .map_err(|err| eyre!("cache-miss-only check failed: {err:?}"))?;

    let (cache_update, next_cache_anchor) = apply_cache_transition_and_check(
        prev_cache,
        &actual_accessed,
        sidecar.block_number,
        sidecar.block_hash,
        sidecar.cache_policy_id,
        sidecar.next_cache_anchor,
    )?;

    let root_witness_completeness =
        root_witness_completeness_from_bundle(&execution_output.state, &sidecar.cache_miss_targets);

    // Advance the trie-node cache only after every sidecar and value-cache check has succeeded.
    // The proof was reconstructed from the sidecar plus the pre-block trie cache above.
    trie_cache.on_block_executed(&witness_multiproof, &expected_miss.accounts, |address| {
        prev_cache.contains_account(address)
    });

    Ok(SidecarReexecReport {
        computed_state_root,
        actual_accessed,
        expected_miss,
        next_cache_anchor,
        cache_update,
        root_witness_completeness,
        trustless_state_root,
    })
}

fn apply_cache_transition_and_check(
    cache: &mut NetworkStateCache,
    accessed: &BlockAccessedState,
    block_number: u64,
    block_hash: B256,
    cache_policy_id: B256,
    expected_next_anchor: CacheAnchor,
) -> Result<(UpdateStats, CacheAnchor)> {
    let cache_update = cache.on_block_executed(block_number, accessed);
    let next_cache_anchor = cache.cache_anchor(block_number, block_hash, cache_policy_id);
    if next_cache_anchor != expected_next_anchor {
        cache.rollback_block(block_number).map_err(|rollback_err| {
            eyre!("next cache anchor mismatch; cache rollback also failed: {rollback_err}")
        })?;
        bail!(
            "next cache anchor mismatch: expected {expected_next_anchor:?}, got {next_cache_anchor:?}"
        );
    }
    Ok((cache_update, next_cache_anchor))
}

fn prefilter(
    block: &RecoveredBlock<BlockTy<EthPrimitives>>,
    prev_cache: &NetworkStateCache,
    sidecar: &PartialStatelessSidecar,
) -> Result<()> {
    if sidecar.block_hash != block.hash() {
        bail!("sidecar block_hash mismatch");
    }
    if sidecar.parent_hash != block.parent_hash() {
        bail!("sidecar parent_hash mismatch");
    }
    if sidecar.block_number != block.number() {
        bail!("sidecar block_number mismatch");
    }

    let local_prev_anchor =
        prev_cache.cache_anchor(sidecar.cache_block, sidecar.parent_hash, sidecar.cache_policy_id);
    check_sidecar_context(sidecar, &local_prev_anchor)
        .map_err(|err| eyre!("sidecar cache context mismatch: {err:?}"))?;

    Ok(())
}

struct WitnessBackedStateProvider<'a> {
    cache: &'a NetworkStateCache,
    witness_accounts: HashMap<Address, Option<Account>>,
    witness_storage: HashMap<(Address, B256), U256>,
    witness_codes: HashMap<B256, Bytes>,
    witness_headers: HashMap<u64, B256>,
    block_number: u64,
}

impl WitnessBackedStateProvider<'_> {
    fn missing(label: &str) -> ProviderError {
        ProviderError::TrieWitnessError(label.to_string())
    }
}

impl EvmStateProvider for WitnessBackedStateProvider<'_> {
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        if let Some(entry) = self.cache.accounts().get(address) {
            return Ok(Some(Account {
                nonce: entry.value.nonce,
                balance: entry.value.balance,
                bytecode_hash: entry.value.code_hash,
            }));
        }

        if let Some(account) = self.witness_accounts.get(address) {
            return Ok(*account);
        }

        Err(Self::missing(&format!("missing account witness for {address:?}")))
    }

    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        if number >= self.block_number || number.saturating_add(256) < self.block_number {
            return Ok(None);
        }

        self.witness_headers
            .get(&number)
            .copied()
            .map(Some)
            .ok_or_else(|| Self::missing(&format!("missing ancestor header witness for {number}")))
    }

    fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
        if let Some(entry) = self.cache.codes().get(code_hash) {
            return Ok(Some(Bytecode::new_raw(entry.value.clone())));
        }

        if let Some(code) = self.witness_codes.get(code_hash) {
            return Ok(Some(Bytecode::new_raw(code.clone())));
        }

        Err(Self::missing(&format!("missing bytecode witness for {code_hash:?}")))
    }

    fn storage(&self, account: Address, storage_key: B256) -> ProviderResult<Option<U256>> {
        if let Some(entry) = self.cache.storage().get(&(account, storage_key)) {
            return Ok(Some(entry.value));
        }

        if let Some(value) = self.witness_storage.get(&(account, storage_key)) {
            return Ok(Some(*value));
        }

        Err(Self::missing(&format!(
            "missing storage witness for account={account:?}, slot={storage_key:?}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use partial_stateless::policy::{AccountData, LastNBlocksPolicy};

    #[test]
    fn next_anchor_mismatch_rolls_back_cache_transition() {
        let mut cache = NetworkStateCache::new(
            Box::new(LastNBlocksPolicy::new(60)),
            Box::new(LastNBlocksPolicy::new(30)),
        );
        cache.on_block_executed(99, &BlockAccessedState::default());
        let root_before = cache.cache_root();
        let address = Address::repeat_byte(0x11);
        let mut accessed = BlockAccessedState::default();
        accessed
            .accounts
            .insert(address, AccountData { nonce: 1, balance: U256::from(10), code_hash: None });

        let _error = apply_cache_transition_and_check(
            &mut cache,
            &accessed,
            100,
            B256::repeat_byte(0x22),
            B256::repeat_byte(0x33),
            CacheAnchor {
                block_number: 100,
                block_hash: B256::repeat_byte(0x22),
                cache_policy_id: B256::repeat_byte(0x33),
                cache_root: B256::ZERO,
            },
        )
        .expect_err("wrong next cache root must fail");

        assert_eq!(cache.current_block(), 99);
        assert_eq!(cache.cache_root(), root_before);
        assert!(!cache.contains_account(&address));
    }
}

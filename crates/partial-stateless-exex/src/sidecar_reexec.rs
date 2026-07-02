use crate::CacheConfig;
use alloy_primitives::{Address, Bytes, B256, U256};
use eyre::{bail, eyre, Result};
use partial_stateless::{
    accessed_state::BlockAccessedState,
    check_next_cache_anchor, check_sidecar_context, check_sidecar_miss_targets,
    network_cache::NetworkStateCache,
    policy::LastNBlocksPolicy,
    witness_check::{
        materialize_sidecar_witness_with_limits, root_witness_completeness_from_bundle,
        SidecarWitnessCheckLimits,
    },
    CacheAnchor, PartialStatelessSidecar, RootWitnessCompletenessReport, StateTargetSet,
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
    pub root_witness_completeness: RootWitnessCompletenessReport,
}

pub(crate) fn check_provider_assisted_sidecar<Evm>(
    evm_config: &Evm,
    full_provider: &dyn StateProvider,
    block: &RecoveredBlock<BlockTy<EthPrimitives>>,
    prev_cache: &NetworkStateCache,
    sidecar: &PartialStatelessSidecar,
    config: &CacheConfig,
    limits: &SidecarReexecLimits,
) -> Result<SidecarReexecReport>
where
    Evm: ConfigureEvm<Primitives = EthPrimitives>,
{
    prefilter(block, prev_cache, sidecar)?;

    let materialized = materialize_sidecar_witness_with_limits(sidecar, limits)
        .map_err(|err| eyre!("sidecar witness check failed: {err}"))?;
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

    let mut next_cache = clone_cache(prev_cache, config);
    next_cache.on_block_executed(sidecar.block_number, &actual_accessed);
    let next_cache_anchor =
        next_cache.cache_anchor(sidecar.block_number, sidecar.block_hash, sidecar.cache_policy_id);
    check_next_cache_anchor(sidecar, &next_cache_anchor)
        .map_err(|err| eyre!("next cache anchor mismatch: {err:?}"))?;

    let root_witness_completeness =
        root_witness_completeness_from_bundle(&execution_output.state, &sidecar.cache_miss_targets);

    Ok(SidecarReexecReport {
        computed_state_root,
        actual_accessed,
        expected_miss,
        next_cache_anchor,
        root_witness_completeness,
    })
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

fn clone_cache(cache: &NetworkStateCache, config: &CacheConfig) -> NetworkStateCache {
    NetworkStateCache::restore(
        cache.accounts().clone(),
        cache.storage().clone(),
        cache.codes().clone(),
        cache.current_block(),
        Box::new(LastNBlocksPolicy::new(config.account_window)),
        Box::new(LastNBlocksPolicy::new(config.storage_window)),
    )
}

//! Block access list helpers for RPC cache prewarming.

use crate::cache::db::StateProviderTraitObjWrapper;
use alloy_consensus::BlockHeader;
use alloy_eips::eip7928::{bal::DecodedBal, BlockAccessList};
use alloy_primitives::Bytes;
use reth_errors::ProviderError;
use reth_evm::{block::BlockExecutor, ConfigureEvm, Evm};
use reth_primitives_traits::{BlockTy, RecoveredBlock};
use reth_revm::{database::StateProviderDatabase, State};
use reth_storage_api::{NodePrimitivesProvider, StateProviderFactory};

/// Recomputes the BAL for a block by replaying it on top of its parent state.
pub fn build_bal_for_block<Provider, EvmConfig>(
    provider: &Provider,
    evm_config: &EvmConfig,
    block: &RecoveredBlock<BlockTy<Provider::Primitives>>,
) -> Result<BlockAccessList, ProviderError>
where
    Provider: NodePrimitivesProvider + StateProviderFactory,
    EvmConfig: ConfigureEvm<Primitives = Provider::Primitives>,
{
    let state = provider.state_by_block_id(block.parent_hash().into())?;
    let mut db = State::builder()
        .with_database(StateProviderDatabase::new(StateProviderTraitObjWrapper(state)))
        .with_bal_builder()
        .build();

    let mut executor = evm_config
        .executor_for_block(&mut db, block.sealed_block())
        .map_err(ProviderError::other)?;

    executor.apply_pre_execution_changes().map_err(ProviderError::other)?;
    executor.evm_mut().db_mut().bump_bal_index();

    for tx in block.transactions_recovered() {
        executor.execute_transaction(tx).map_err(ProviderError::other)?;
        executor.evm_mut().db_mut().bump_bal_index();
    }

    executor.apply_post_execution_changes().map_err(ProviderError::other)?;

    Ok(db.take_built_alloy_bal().expect("BAL builder configured"))
}

/// Recomputes the revm BAL for a block by replaying it on top of its parent state.
pub fn build_revm_bal_for_block<Provider, EvmConfig>(
    provider: &Provider,
    evm_config: &EvmConfig,
    block: &RecoveredBlock<BlockTy<Provider::Primitives>>,
) -> Result<DecodedBal<revm::database::state::bal::Bal>, ProviderError>
where
    Provider: NodePrimitivesProvider + StateProviderFactory,
    EvmConfig: ConfigureEvm<Primitives = Provider::Primitives>,
{
    let bal = build_bal_for_block(provider, evm_config, block)?;
    let raw = Bytes::from(alloy_rlp::encode(&bal));
    let revm_bal = bal.try_into().map_err(ProviderError::other)?;

    Ok(DecodedBal::new(revm_bal, raw))
}

//! [EIP-2935](https://eips.ethereum.org/EIPS/eip-2935) system call implementation.

use alloc::{boxed::Box, string::ToString};
use alloy_eips::eip2935::HISTORY_STORAGE_ADDRESS;

use crate::ConfigureEvm;
use core::fmt::Display;
use reth_chainspec::{ChainSpec, EthereumHardforks};
use reth_execution_errors::{BlockExecutionError, BlockValidationError};
use revm::{interpreter::Host, Database, DatabaseCommit, Evm};
use revm_primitives::{BlockEnv, CfgEnvWithHandlerCfg, EnvWithHandlerCfg, ResultAndState, B256};

/// Apply the [EIP-2935](https://eips.ethereum.org/EIPS/eip-2935) pre block contract call.
///
/// This constructs a new [`Evm`] with the given database and environment ([`CfgEnvWithHandlerCfg`]
/// and [`BlockEnv`]) to execute the pre block contract call.
///
/// This uses [`apply_blockhashes_contract_call`] to ultimately apply the
/// blockhash contract state change.
pub fn pre_block_blockhashes_contract_call<EvmConfig, DB>(
    db: &mut DB,
    evm_config: &EvmConfig,
    chain_spec: &ChainSpec,
    initialized_cfg: &CfgEnvWithHandlerCfg,
    initialized_block_env: &BlockEnv,
    parent_block_hash: B256,
) -> Result<(), BlockExecutionError>
where
    DB: Database + DatabaseCommit,
    DB::Error: Display,
    EvmConfig: ConfigureEvm,
{
    // Apply the pre-block EIP-2935 contract call
    let mut evm_pre_block = Evm::builder()
        .with_db(db)
        .with_env_with_handler_cfg(EnvWithHandlerCfg::new_with_cfg_env(
            initialized_cfg.clone(),
            initialized_block_env.clone(),
            Default::default(),
        ))
        .build();

    apply_blockhashes_contract_call(
        evm_config,
        chain_spec,
        initialized_block_env.timestamp.to(),
        initialized_block_env.number.to(),
        parent_block_hash,
        &mut evm_pre_block,
    )
}

/// Applies the pre-block call to the [EIP-2935] blockhashes contract, using the given block,
/// [`ChainSpec`], and EVM.
///
/// If Prague is not activated, or the block is the genesis block, then this is a no-op, and no
/// state changes are made.
///
/// Note: this does not commit the state changes to the database, it only transact the call.
///
/// Returns `None` if Prague is not active or the block is the genesis block, otherwise returns the
/// result of the call.
///
/// [EIP-2935]: https://eips.ethereum.org/EIPS/eip-2935
#[inline]
pub fn transact_blockhashes_contract_call<EvmConfig, EXT, DB>(
    evm_config: &EvmConfig,
    chain_spec: &ChainSpec,
    block_timestamp: u64,
    block_number: u64,
    parent_block_hash: B256,
    evm: &mut Evm<'_, EXT, DB>,
) -> Result<Option<ResultAndState>, BlockExecutionError>
where
    DB: Database + DatabaseCommit,
    DB::Error: core::fmt::Display,
    EvmConfig: ConfigureEvm,
{
    if !chain_spec.is_prague_active_at_timestamp(block_timestamp) {
        return Ok(None)
    }

    // if the block number is zero (genesis block) then no system transaction may occur as per
    // EIP-2935
    if block_number == 0 {
        return Ok(None)
    }

    // get previous env
    let previous_env = Box::new(evm.context.env().clone());

    // modify env for pre block call
    evm_config.fill_tx_env_system_contract_call(
        &mut evm.context.evm.env,
        alloy_eips::eip4788::SYSTEM_ADDRESS,
        HISTORY_STORAGE_ADDRESS,
        parent_block_hash.0.into(),
    );

    let mut res = match evm.transact() {
        Ok(res) => res,
        Err(e) => {
            evm.context.evm.env = previous_env;
            return Err(BlockValidationError::BlockHashContractCall { message: e.to_string() }.into())
        }
    };

    res.state.remove(&alloy_eips::eip4788::SYSTEM_ADDRESS);
    res.state.remove(&evm.block().coinbase);

    // re-set the previous env
    evm.context.evm.env = previous_env;

    Ok(Some(res))
}

/// Applies the pre-block call to the [EIP-2935] blockhashes contract, using the given block,
/// [`ChainSpec`], and EVM and commits the relevant state changes.
///
/// If Prague is not activated, or the block is the genesis block, then this is a no-op, and no
/// state changes are made.
///
/// [EIP-2935]: https://eips.ethereum.org/EIPS/eip-2935
#[inline]
pub fn apply_blockhashes_contract_call<EvmConfig, EXT, DB>(
    evm_config: &EvmConfig,
    chain_spec: &ChainSpec,
    block_timestamp: u64,
    block_number: u64,
    parent_block_hash: B256,
    evm: &mut Evm<'_, EXT, DB>,
) -> Result<(), BlockExecutionError>
where
    DB: Database + DatabaseCommit,
    DB::Error: core::fmt::Display,
    EvmConfig: ConfigureEvm,
{
    if let Some(res) = transact_blockhashes_contract_call(
        evm_config,
        chain_spec,
        block_timestamp,
        block_number,
        parent_block_hash,
        evm,
    )? {
        evm.context.evm.db.commit(res.state);
    }

    Ok(())
}

//! System contract call functions.

#[cfg(feature = "std")]
use std::fmt::Display;
#[cfg(not(feature = "std"))]
use {
    alloc::{boxed::Box, format, string::ToString, vec::Vec},
    core::fmt::Display,
};

use crate::ConfigureEvm;
use alloy_eips::{
    eip2935::HISTORY_STORAGE_ADDRESS,
    eip4788::BEACON_ROOTS_ADDRESS,
    eip7002::{WithdrawalRequest, WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS},
    eip7251::{ConsolidationRequest, CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS},
};
use reth_chainspec::{ChainSpec, EthereumHardforks};
use reth_execution_errors::{BlockExecutionError, BlockValidationError};
use reth_primitives::{Buf, Request};
use revm::{interpreter::Host, Database, DatabaseCommit, Evm};
use revm_primitives::{
    Address, BlockEnv, Bytes, CfgEnvWithHandlerCfg, EnvWithHandlerCfg, ExecutionResult, FixedBytes,
    ResultAndState, B256,
};

/// Apply the [EIP-2935](https://eips.ethereum.org/EIPS/eip-2935) pre block contract call.
///
/// This constructs a new [`Evm`] with the given database and environment ([`CfgEnvWithHandlerCfg`]
/// and [`BlockEnv`]) to execute the pre block contract call.
///
/// This uses [`apply_blockhashes_contract_call`] to ultimately apply the blockhash contract state
/// change.
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
    if !chain_spec.is_prague_active_at_timestamp(block_timestamp) {
        return Ok(())
    }

    // if the block number is zero (genesis block) then no system transaction may occur as per
    // EIP-2935
    if block_number == 0 {
        return Ok(())
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

    let mut state = match evm.transact() {
        Ok(res) => res.state,
        Err(e) => {
            evm.context.evm.env = previous_env;
            return Err(BlockValidationError::BlockHashContractCall { message: e.to_string() }.into())
        }
    };

    state.remove(&alloy_eips::eip4788::SYSTEM_ADDRESS);
    state.remove(&evm.block().coinbase);

    evm.context.evm.db.commit(state);

    // re-set the previous env
    evm.context.evm.env = previous_env;

    Ok(())
}

/// Apply the [EIP-4788](https://eips.ethereum.org/EIPS/eip-4788) pre block contract call.
///
/// This constructs a new [`Evm`] with the given DB, and environment
/// ([`CfgEnvWithHandlerCfg`] and [`BlockEnv`]) to execute the pre block contract call.
///
/// This uses [`apply_beacon_root_contract_call`] to ultimately apply the beacon root contract state
/// change.
pub fn pre_block_beacon_root_contract_call<EvmConfig, DB>(
    db: &mut DB,
    evm_config: &EvmConfig,
    chain_spec: &ChainSpec,
    initialized_cfg: &CfgEnvWithHandlerCfg,
    initialized_block_env: &BlockEnv,
    parent_beacon_block_root: Option<B256>,
) -> Result<(), BlockExecutionError>
where
    DB: Database + DatabaseCommit,
    DB::Error: Display,
    EvmConfig: ConfigureEvm,
{
    // apply pre-block EIP-4788 contract call
    let mut evm_pre_block = Evm::builder()
        .with_db(db)
        .with_env_with_handler_cfg(EnvWithHandlerCfg::new_with_cfg_env(
            initialized_cfg.clone(),
            initialized_block_env.clone(),
            Default::default(),
        ))
        .build();

    // initialize a block from the env, because the pre block call needs the block itself
    apply_beacon_root_contract_call(
        evm_config,
        chain_spec,
        initialized_block_env.timestamp.to(),
        initialized_block_env.number.to(),
        parent_beacon_block_root,
        &mut evm_pre_block,
    )
}

/// Applies the pre-block call to the [EIP-4788] beacon block root contract, using the given block,
/// [`ChainSpec`], EVM.
///
/// If Cancun is not activated or the block is the genesis block, then this is a no-op, and no
/// state changes are made.
///
/// [EIP-4788]: https://eips.ethereum.org/EIPS/eip-4788
#[inline]
pub fn apply_beacon_root_contract_call<EvmConfig, EXT, DB>(
    evm_config: &EvmConfig,
    chain_spec: &ChainSpec,
    block_timestamp: u64,
    block_number: u64,
    parent_beacon_block_root: Option<B256>,
    evm: &mut Evm<'_, EXT, DB>,
) -> Result<(), BlockExecutionError>
where
    DB: Database + DatabaseCommit,
    DB::Error: core::fmt::Display,
    EvmConfig: ConfigureEvm,
{
    if !chain_spec.is_cancun_active_at_timestamp(block_timestamp) {
        return Ok(())
    }

    let parent_beacon_block_root =
        parent_beacon_block_root.ok_or(BlockValidationError::MissingParentBeaconBlockRoot)?;

    // if the block number is zero (genesis block) then the parent beacon block root must
    // be 0x0 and no system transaction may occur as per EIP-4788
    if block_number == 0 {
        if !parent_beacon_block_root.is_zero() {
            return Err(BlockValidationError::CancunGenesisParentBeaconBlockRootNotZero {
                parent_beacon_block_root,
            }
            .into())
        }
        return Ok(())
    }

    // get previous env
    let previous_env = Box::new(evm.context.env().clone());

    // modify env for pre block call
    evm_config.fill_tx_env_system_contract_call(
        &mut evm.context.evm.env,
        alloy_eips::eip4788::SYSTEM_ADDRESS,
        BEACON_ROOTS_ADDRESS,
        parent_beacon_block_root.0.into(),
    );

    let mut state = match evm.transact() {
        Ok(res) => res.state,
        Err(e) => {
            evm.context.evm.env = previous_env;
            return Err(BlockValidationError::BeaconRootContractCall {
                parent_beacon_block_root: Box::new(parent_beacon_block_root),
                message: e.to_string(),
            }
            .into())
        }
    };

    state.remove(&alloy_eips::eip4788::SYSTEM_ADDRESS);
    state.remove(&evm.block().coinbase);

    evm.context.evm.db.commit(state);

    // re-set the previous env
    evm.context.evm.env = previous_env;

    Ok(())
}

/// Apply the [EIP-7002](https://eips.ethereum.org/EIPS/eip-7002) post block contract call.
///
/// This constructs a new [Evm] with the given DB, and environment
/// ([`CfgEnvWithHandlerCfg`] and [`BlockEnv`]) to execute the post block contract call.
///
/// This uses [`apply_withdrawal_requests_contract_call`] to ultimately calculate the
/// [requests](Request).
pub fn post_block_withdrawal_requests_contract_call<EvmConfig, DB>(
    evm_config: &EvmConfig,
    db: &mut DB,
    initialized_cfg: &CfgEnvWithHandlerCfg,
    initialized_block_env: &BlockEnv,
) -> Result<Vec<Request>, BlockExecutionError>
where
    DB: Database + DatabaseCommit,
    DB::Error: Display,
    EvmConfig: ConfigureEvm,
{
    // apply post-block EIP-7002 contract call
    let mut evm_post_block = Evm::builder()
        .with_db(db)
        .with_env_with_handler_cfg(EnvWithHandlerCfg::new_with_cfg_env(
            initialized_cfg.clone(),
            initialized_block_env.clone(),
            Default::default(),
        ))
        .build();

    // initialize a block from the env, because the post block call needs the block itself
    apply_withdrawal_requests_contract_call(evm_config, &mut evm_post_block)
}

/// Applies the post-block call to the EIP-7002 withdrawal requests contract.
///
/// If Prague is not active at the given timestamp, then this is a no-op, and an empty vector is
/// returned. Otherwise, the withdrawal requests are returned.
#[inline]
pub fn apply_withdrawal_requests_contract_call<EvmConfig, EXT, DB>(
    evm_config: &EvmConfig,
    evm: &mut Evm<'_, EXT, DB>,
) -> Result<Vec<Request>, BlockExecutionError>
where
    DB: Database + DatabaseCommit,
    DB::Error: core::fmt::Display,
    EvmConfig: ConfigureEvm,
{
    // get previous env
    let previous_env = Box::new(evm.context.env().clone());

    // Fill transaction environment with the EIP-7002 withdrawal requests contract message data.
    //
    // This requirement for the withdrawal requests contract call defined by
    // [EIP-7002](https://eips.ethereum.org/EIPS/eip-7002) is:
    //
    // At the end of processing any execution block where `block.timestamp >= FORK_TIMESTAMP` (i.e.
    // after processing all transactions and after performing the block body withdrawal requests
    // validations), call the contract as `SYSTEM_ADDRESS`.
    evm_config.fill_tx_env_system_contract_call(
        &mut evm.context.evm.env,
        alloy_eips::eip7002::SYSTEM_ADDRESS,
        WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS,
        Bytes::new(),
    );

    let ResultAndState { result, mut state } = match evm.transact() {
        Ok(res) => res,
        Err(e) => {
            evm.context.evm.env = previous_env;
            return Err(BlockValidationError::WithdrawalRequestsContractCall {
                message: format!("execution failed: {e}"),
            }
            .into())
        }
    };

    // cleanup the state
    state.remove(&alloy_eips::eip7002::SYSTEM_ADDRESS);
    state.remove(&evm.block().coinbase);
    evm.context.evm.db.commit(state);

    // re-set the previous env
    evm.context.evm.env = previous_env;

    let mut data = match result {
        ExecutionResult::Success { output, .. } => Ok(output.into_data()),
        ExecutionResult::Revert { output, .. } => {
            Err(BlockValidationError::WithdrawalRequestsContractCall {
                message: format!("execution reverted: {output}"),
            })
        }
        ExecutionResult::Halt { reason, .. } => {
            Err(BlockValidationError::WithdrawalRequestsContractCall {
                message: format!("execution halted: {reason:?}"),
            })
        }
    }?;

    // Withdrawals are encoded as a series of withdrawal requests, each with the following
    // format:
    //
    // +------+--------+--------+
    // | addr | pubkey | amount |
    // +------+--------+--------+
    //    20      48        8

    const WITHDRAWAL_REQUEST_SIZE: usize = 20 + 48 + 8;
    let mut withdrawal_requests = Vec::with_capacity(data.len() / WITHDRAWAL_REQUEST_SIZE);
    while data.has_remaining() {
        if data.remaining() < WITHDRAWAL_REQUEST_SIZE {
            return Err(BlockValidationError::WithdrawalRequestsContractCall {
                message: "invalid withdrawal request length".to_string(),
            }
            .into())
        }

        let mut source_address = Address::ZERO;
        data.copy_to_slice(source_address.as_mut_slice());

        let mut validator_pubkey = FixedBytes::<48>::ZERO;
        data.copy_to_slice(validator_pubkey.as_mut_slice());

        let amount = data.get_u64();

        withdrawal_requests
            .push(WithdrawalRequest { source_address, validator_pubkey, amount }.into());
    }

    Ok(withdrawal_requests)
}

/// Apply the [EIP-7251](https://eips.ethereum.org/EIPS/eip-7251) post block contract call.
///
/// This constructs a new [Evm] with the given DB, and environment
/// ([`CfgEnvWithHandlerCfg`] and [`BlockEnv`]) to execute the post block contract call.
///
/// This uses [`apply_consolidation_requests_contract_call`] to ultimately calculate the
/// [requests](Request).
pub fn post_block_consolidation_requests_contract_call<EvmConfig, DB>(
    evm_config: &EvmConfig,
    db: &mut DB,
    initialized_cfg: &CfgEnvWithHandlerCfg,
    initialized_block_env: &BlockEnv,
) -> Result<Vec<Request>, BlockExecutionError>
where
    DB: Database + DatabaseCommit,
    DB::Error: Display,
    EvmConfig: ConfigureEvm,
{
    // apply post-block EIP-7251 contract call
    let mut evm_post_block = Evm::builder()
        .with_db(db)
        .with_env_with_handler_cfg(EnvWithHandlerCfg::new_with_cfg_env(
            initialized_cfg.clone(),
            initialized_block_env.clone(),
            Default::default(),
        ))
        .build();

    // initialize a block from the env, because the post block call needs the block itself
    apply_consolidation_requests_contract_call(evm_config, &mut evm_post_block)
}

/// Applies the post-block call to the EIP-7251 consolidation requests contract.
///
/// If Prague is not active at the given timestamp, then this is a no-op, and an empty vector is
/// returned. Otherwise, the consolidation requests are returned.
#[inline]
pub fn apply_consolidation_requests_contract_call<EvmConfig, EXT, DB>(
    evm_config: &EvmConfig,
    evm: &mut Evm<'_, EXT, DB>,
) -> Result<Vec<Request>, BlockExecutionError>
where
    DB: Database + DatabaseCommit,
    DB::Error: core::fmt::Display,
    EvmConfig: ConfigureEvm,
{
    // get previous env
    let previous_env = Box::new(evm.context.env().clone());

    // Fill transaction environment with the EIP-7251 consolidation requests contract message data.
    //
    // This requirement for the consolidation requests contract call defined by
    // [EIP-7251](https://eips.ethereum.org/EIPS/eip-7251) is:
    //
    // At the end of processing any execution block where block.timestamp >= FORK_TIMESTAMP (i.e.
    // after processing all transactions and after performing the block body requests validations)
    // clienst software MUST [..] call the contract as `SYSTEM_ADDRESS` and empty input data to
    // trigger the system subroutine execute.
    evm_config.fill_tx_env_system_contract_call(
        &mut evm.context.evm.env,
        alloy_eips::eip7002::SYSTEM_ADDRESS,
        CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS,
        Bytes::new(),
    );

    let ResultAndState { result, mut state } = match evm.transact() {
        Ok(res) => res,
        Err(e) => {
            evm.context.evm.env = previous_env;
            return Err(BlockValidationError::ConsolidationRequestsContractCall {
                message: format!("execution failed: {e}"),
            }
            .into())
        }
    };

    // cleanup the state
    state.remove(&alloy_eips::eip7002::SYSTEM_ADDRESS);
    state.remove(&evm.block().coinbase);
    evm.context.evm.db.commit(state);

    // re-set the previous env
    evm.context.evm.env = previous_env;

    let mut data = match result {
        ExecutionResult::Success { output, .. } => Ok(output.into_data()),
        ExecutionResult::Revert { output, .. } => {
            Err(BlockValidationError::ConsolidationRequestsContractCall {
                message: format!("execution reverted: {output}"),
            })
        }
        ExecutionResult::Halt { reason, .. } => {
            Err(BlockValidationError::ConsolidationRequestsContractCall {
                message: format!("execution halted: {reason:?}"),
            })
        }
    }?;

    // Consolidations are encoded as a series of consolidation requests, each with the following
    // format:
    //
    // +------+--------+---------------+
    // | addr | pubkey | target pubkey |
    // +------+--------+---------------+
    //    20      48        48

    const CONSOLIDATION_REQUEST_SIZE: usize = 20 + 48 + 48;
    let mut consolidation_requests = Vec::with_capacity(data.len() / CONSOLIDATION_REQUEST_SIZE);
    while data.has_remaining() {
        if data.remaining() < CONSOLIDATION_REQUEST_SIZE {
            return Err(BlockValidationError::ConsolidationRequestsContractCall {
                message: "invalid consolidation request length".to_string(),
            }
            .into())
        }

        let mut source_address = Address::ZERO;
        data.copy_to_slice(source_address.as_mut_slice());

        let mut source_pubkey = FixedBytes::<48>::ZERO;
        data.copy_to_slice(source_pubkey.as_mut_slice());

        let mut target_pubkey = FixedBytes::<48>::ZERO;
        data.copy_to_slice(target_pubkey.as_mut_slice());

        consolidation_requests.push(Request::ConsolidationRequest(ConsolidationRequest {
            source_address,
            source_pubkey,
            target_pubkey,
        }));
    }

    Ok(consolidation_requests)
}

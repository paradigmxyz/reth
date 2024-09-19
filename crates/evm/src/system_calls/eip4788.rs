//! [EIP-4788](https://eips.ethereum.org/EIPS/eip-4788) system call implementation.
use alloc::{boxed::Box, string::ToString};

use crate::ConfigureEvm;
use alloy_eips::eip4788::BEACON_ROOTS_ADDRESS;
use reth_chainspec::{ChainSpec, EthereumHardforks};
use reth_execution_errors::{BlockExecutionError, BlockValidationError};
use reth_primitives::Header;
use revm::{interpreter::Host, Database, DatabaseCommit, Evm};
use revm_primitives::{BlockEnv, CfgEnvWithHandlerCfg, EnvWithHandlerCfg, B256};

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
    DB::Error: core::fmt::Display,
    EvmConfig: ConfigureEvm<Header = Header>,
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
    EvmConfig: ConfigureEvm<Header = Header>,
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

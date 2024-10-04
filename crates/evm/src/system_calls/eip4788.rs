//! [EIP-4788](https://eips.ethereum.org/EIPS/eip-4788) system call implementation.
use alloc::{boxed::Box, string::ToString};

use crate::ConfigureEvm;
use alloy_eips::eip4788::BEACON_ROOTS_ADDRESS;
use alloy_primitives::B256;
use reth_chainspec::EthereumHardforks;
use reth_execution_errors::{BlockExecutionError, BlockValidationError};
use reth_primitives::Header;
use revm::{interpreter::Host, Database, Evm};
use revm_primitives::ResultAndState;

/// Applies the pre-block call to the [EIP-4788] beacon block root contract, using the given block,
/// chain spec, EVM.
///
/// Note: this does not commit the state changes to the database, it only transact the call.
///
/// Returns `None` if Cancun is not active or the block is the genesis block, otherwise returns the
/// result of the call.
///
/// [EIP-4788]: https://eips.ethereum.org/EIPS/eip-4788
#[inline]
pub(crate) fn transact_beacon_root_contract_call<EvmConfig, EXT, DB, Spec>(
    evm_config: &EvmConfig,
    chain_spec: &Spec,
    block_timestamp: u64,
    block_number: u64,
    parent_beacon_block_root: Option<B256>,
    evm: &mut Evm<'_, EXT, DB>,
) -> Result<Option<ResultAndState>, BlockExecutionError>
where
    DB: Database,
    DB::Error: core::fmt::Display,
    EvmConfig: ConfigureEvm<Header = Header>,
    Spec: EthereumHardforks,
{
    if !chain_spec.is_cancun_active_at_timestamp(block_timestamp) {
        return Ok(None)
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
        return Ok(None)
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

    let mut res = match evm.transact() {
        Ok(res) => res,
        Err(e) => {
            evm.context.evm.env = previous_env;
            return Err(BlockValidationError::BeaconRootContractCall {
                parent_beacon_block_root: Box::new(parent_beacon_block_root),
                message: e.to_string(),
            }
            .into())
        }
    };

    res.state.remove(&alloy_eips::eip4788::SYSTEM_ADDRESS);
    res.state.remove(&evm.block().coinbase);

    // re-set the previous env
    evm.context.evm.env = previous_env;

    Ok(Some(res))
}

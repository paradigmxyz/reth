//! [EIP-2935](https://eips.ethereum.org/EIPS/eip-2935) system call implementation.

use alloc::{boxed::Box, string::ToString};
use alloy_eips::eip2935::HISTORY_STORAGE_ADDRESS;

use crate::ConfigureEvm;
use alloy_primitives::B256;
use reth_chainspec::EthereumHardforks;
use reth_execution_errors::{BlockExecutionError, BlockValidationError};
use reth_primitives::Header;
use revm::{interpreter::Host, Database, Evm};
use revm_primitives::ResultAndState;

/// Applies the pre-block call to the [EIP-2935] blockhashes contract, using the given block,
/// chain specification, and EVM.
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
pub(crate) fn transact_blockhashes_contract_call<EvmConfig, EXT, DB>(
    evm_config: &EvmConfig,
    chain_spec: impl EthereumHardforks,
    block_timestamp: u64,
    block_number: u64,
    parent_block_hash: B256,
    evm: &mut Evm<'_, EXT, DB>,
) -> Result<Option<ResultAndState>, BlockExecutionError>
where
    DB: Database,
    DB::Error: core::fmt::Display,
    EvmConfig: ConfigureEvm<Header = Header>,
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

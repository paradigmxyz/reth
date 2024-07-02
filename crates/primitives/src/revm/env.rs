use crate::{
    revm_primitives::{Env, TxEnv},
    Address, Bytes, TxKind, B256, U256,
};

use alloy_eips::{eip4788::BEACON_ROOTS_ADDRESS, eip7002::WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS};
#[cfg(feature = "optimism")]
use revm_primitives::OptimismFields;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

/// Fill transaction environment with the EIP-4788 system contract message data.
///
/// This requirements for the beacon root contract call defined by
/// [EIP-4788](https://eips.ethereum.org/EIPS/eip-4788) are:
///
/// At the start of processing any execution block where `block.timestamp >= FORK_TIMESTAMP` (i.e.
/// before processing any transactions), call [`BEACON_ROOTS_ADDRESS`] as
/// [`SYSTEM_ADDRESS`](alloy_eips::eip4788::SYSTEM_ADDRESS) with the 32-byte input of
/// `header.parent_beacon_block_root`. This will trigger the `set()` routine of the beacon roots
/// contract.
pub fn fill_tx_env_with_beacon_root_contract_call(env: &mut Env, parent_beacon_block_root: B256) {
    fill_tx_env_with_system_contract_call(
        env,
        alloy_eips::eip4788::SYSTEM_ADDRESS,
        BEACON_ROOTS_ADDRESS,
        parent_beacon_block_root.0.into(),
    );
}

/// Fill transaction environment with the EIP-7002 withdrawal requests contract message data.
//
/// This requirement for the withdrawal requests contract call defined by
/// [EIP-7002](https://eips.ethereum.org/EIPS/eip-7002) is:
//
/// At the end of processing any execution block where `block.timestamp >= FORK_TIMESTAMP` (i.e.
/// after processing all transactions and after performing the block body withdrawal requests
/// validations), call the contract as `SYSTEM_ADDRESS`.
pub fn fill_tx_env_with_withdrawal_requests_contract_call(env: &mut Env) {
    fill_tx_env_with_system_contract_call(
        env,
        alloy_eips::eip7002::SYSTEM_ADDRESS,
        WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS,
        Bytes::new(),
    );
}

/// Fill transaction environment with the system caller and the system contract address and message
/// data.
///
/// This is a system operation and therefore:
///  * the call must execute to completion
///  * the call does not count against the block’s gas limit
///  * the call does not follow the EIP-1559 burn semantics - no value should be transferred as part
///    of the call
///  * if no code exists at the provided address, the call will fail silently
fn fill_tx_env_with_system_contract_call(
    env: &mut Env,
    caller: Address,
    contract: Address,
    data: Bytes,
) {
    env.tx = TxEnv {
        caller,
        transact_to: TxKind::Call(contract),
        // Explicitly set nonce to None so revm does not do any nonce checks
        nonce: None,
        gas_limit: 30_000_000,
        value: U256::ZERO,
        data,
        // Setting the gas price to zero enforces that no value is transferred as part of the call,
        // and that the call will not count against the block's gas limit
        gas_price: U256::ZERO,
        // The chain ID check is not relevant here and is disabled if set to None
        chain_id: None,
        // Setting the gas priority fee to None ensures the effective gas price is derived from the
        // `gas_price` field, which we need to be zero
        gas_priority_fee: None,
        access_list: Vec::new(),
        // blob fields can be None for this tx
        blob_hashes: Vec::new(),
        max_fee_per_blob_gas: None,
        #[cfg(feature = "optimism")]
        optimism: OptimismFields {
            source_hash: None,
            mint: None,
            is_system_transaction: Some(false),
            // The L1 fee is not charged for the EIP-4788 transaction, submit zero bytes for the
            // enveloped tx size.
            enveloped_tx: Some(Bytes::default()),
        },
    };

    // ensure the block gas limit is >= the tx
    env.block.gas_limit = U256::from(env.tx.gas_limit);

    // disable the base fee check for this call by setting the base fee to zero
    env.block.basefee = U256::ZERO;
}

use alloy_eips::{
    eip2935::{HISTORY_STORAGE_ADDRESS, HISTORY_STORAGE_CODE},
    eip7002::WithdrawalRequest,
};
use alloy_rlp::Buf;
use reth_chainspec::ChainSpec;
use reth_consensus_common::calc;
use reth_execution_errors::{BlockExecutionError, BlockValidationError};
use reth_primitives::{
    revm::env::{
        fill_tx_env_with_beacon_root_contract_call,
        fill_tx_env_with_withdrawal_requests_contract_call,
    },
    Address, Header, Request, Withdrawal, B256, U256,
};
use reth_storage_errors::provider::ProviderError;
use revm::{
    interpreter::Host,
    primitives::{
        Account, AccountInfo, Bytecode, EvmStorageSlot, ExecutionResult, FixedBytes, ResultAndState,
    },
    Database, DatabaseCommit, Evm,
};
use std::collections::HashMap;

/// Collect all balance changes at the end of the block.
///
/// Balance changes might include the block reward, uncle rewards, withdrawals, or irregular
/// state changes (DAO fork).
#[allow(clippy::too_many_arguments)]
#[inline]
pub fn post_block_balance_increments(
    chain_spec: &ChainSpec,
    block_number: u64,
    block_difficulty: U256,
    beneficiary: Address,
    block_timestamp: u64,
    total_difficulty: U256,
    ommers: &[Header],
    withdrawals: Option<&[Withdrawal]>,
) -> HashMap<Address, u128> {
    let mut balance_increments = HashMap::new();

    // Add block rewards if they are enabled.
    if let Some(base_block_reward) =
        calc::base_block_reward(chain_spec, block_number, block_difficulty, total_difficulty)
    {
        // Ommer rewards
        for ommer in ommers {
            *balance_increments.entry(ommer.beneficiary).or_default() +=
                calc::ommer_reward(base_block_reward, block_number, ommer.number);
        }

        // Full block reward
        *balance_increments.entry(beneficiary).or_default() +=
            calc::block_reward(base_block_reward, ommers.len());
    }

    // process withdrawals
    insert_post_block_withdrawals_balance_increments(
        chain_spec,
        block_timestamp,
        withdrawals,
        &mut balance_increments,
    );

    balance_increments
}

/// todo: temporary move over of constants from revm until we've migrated to the latest version
pub const HISTORY_SERVE_WINDOW: u64 = 8192;

/// Applies the pre-block state change outlined in [EIP-2935] to store historical blockhashes in a
/// system contract.
///
/// If Prague is not activated, or the block is the genesis block, then this is a no-op, and no
/// state changes are made.
///
/// If the provided block is after Prague has been activated, the parent hash will be inserted.
///
/// [EIP-2935]: https://eips.ethereum.org/EIPS/eip-2935
#[inline]
pub fn apply_blockhashes_update<DB: Database<Error = ProviderError> + DatabaseCommit>(
    db: &mut DB,
    chain_spec: &ChainSpec,
    block_timestamp: u64,
    block_number: u64,
    parent_block_hash: B256,
) -> Result<(), BlockExecutionError>
where
    DB::Error: std::fmt::Display,
{
    // If Prague is not activated or this is the genesis block, no hashes are added.
    if !chain_spec.is_prague_active_at_timestamp(block_timestamp) || block_number == 0 {
        return Ok(())
    }
    assert!(block_number > 0);

    // Account is expected to exist either in genesis (for tests) or deployed on mainnet or
    // testnets.
    // If the account for any reason does not exist, we create it with the EIP-2935 bytecode and a
    // nonce of 1, so it does not get deleted.
    let mut account: Account = db
        .basic(HISTORY_STORAGE_ADDRESS)
        .map_err(BlockValidationError::BlockHashAccountLoadingFailed)?
        .unwrap_or_else(|| AccountInfo {
            nonce: 1,
            code: Some(Bytecode::new_raw(HISTORY_STORAGE_CODE.clone())),
            ..Default::default()
        })
        .into();

    // Insert the state change for the slot
    let (slot, value) = eip2935_block_hash_slot(db, block_number - 1, parent_block_hash)?;
    account.storage.insert(slot, value);

    // Mark the account as touched and commit the state change
    account.mark_touch();
    db.commit(HashMap::from([(HISTORY_STORAGE_ADDRESS, account)]));

    Ok(())
}

/// Helper function to create a [`EvmStorageSlot`] for [EIP-2935] state transitions for a given
/// block number.
///
/// This calculates the correct storage slot in the `BLOCKHASH` history storage address, fetches the
/// blockhash and creates a [`EvmStorageSlot`] with appropriate previous and new values.
fn eip2935_block_hash_slot<DB: Database<Error = ProviderError>>(
    db: &mut DB,
    block_number: u64,
    block_hash: B256,
) -> Result<(U256, EvmStorageSlot), BlockValidationError> {
    let slot = U256::from(block_number % HISTORY_SERVE_WINDOW);
    let current_hash = db
        .storage(HISTORY_STORAGE_ADDRESS, slot)
        .map_err(BlockValidationError::BlockHashAccountLoadingFailed)?;

    Ok((slot, EvmStorageSlot::new_changed(current_hash, block_hash.into())))
}

/// Applies the pre-block call to the [EIP-4788] beacon block root contract, using the given block,
/// [`ChainSpec`], EVM.
///
/// If Cancun is not activated or the block is the genesis block, then this is a no-op, and no
/// state changes are made.
///
/// [EIP-4788]: https://eips.ethereum.org/EIPS/eip-4788
#[inline]
pub fn apply_beacon_root_contract_call<EXT, DB: Database + DatabaseCommit>(
    chain_spec: &ChainSpec,
    block_timestamp: u64,
    block_number: u64,
    parent_beacon_block_root: Option<B256>,
    evm: &mut Evm<'_, EXT, DB>,
) -> Result<(), BlockExecutionError>
where
    DB::Error: std::fmt::Display,
{
    if !chain_spec.is_cancun_active_at_timestamp(block_timestamp) {
        return Ok(())
    }

    let parent_beacon_block_root =
        parent_beacon_block_root.ok_or(BlockValidationError::MissingParentBeaconBlockRoot)?;

    // if the block number is zero (genesis block) then the parent beacon block root must
    // be 0x0 and no system transaction may occur as per EIP-4788
    if block_number == 0 {
        if parent_beacon_block_root != B256::ZERO {
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
    fill_tx_env_with_beacon_root_contract_call(&mut evm.context.evm.env, parent_beacon_block_root);

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

/// Returns a map of addresses to their balance increments if the Shanghai hardfork is active at the
/// given timestamp.
///
/// Zero-valued withdrawals are filtered out.
#[inline]
pub fn post_block_withdrawals_balance_increments(
    chain_spec: &ChainSpec,
    block_timestamp: u64,
    withdrawals: &[Withdrawal],
) -> HashMap<Address, u128> {
    let mut balance_increments = HashMap::with_capacity(withdrawals.len());
    insert_post_block_withdrawals_balance_increments(
        chain_spec,
        block_timestamp,
        Some(withdrawals),
        &mut balance_increments,
    );
    balance_increments
}

/// Applies all withdrawal balance increments if shanghai is active at the given timestamp to the
/// given `balance_increments` map.
///
/// Zero-valued withdrawals are filtered out.
#[inline]
pub fn insert_post_block_withdrawals_balance_increments(
    chain_spec: &ChainSpec,
    block_timestamp: u64,
    withdrawals: Option<&[Withdrawal]>,
    balance_increments: &mut HashMap<Address, u128>,
) {
    // Process withdrawals
    if chain_spec.is_shanghai_active_at_timestamp(block_timestamp) {
        if let Some(withdrawals) = withdrawals {
            for withdrawal in withdrawals {
                if withdrawal.amount > 0 {
                    *balance_increments.entry(withdrawal.address).or_default() +=
                        withdrawal.amount_wei().to::<u128>();
                }
            }
        }
    }
}

/// Applies the post-block call to the EIP-7002 withdrawal requests contract.
///
/// If Prague is not active at the given timestamp, then this is a no-op, and an empty vector is
/// returned. Otherwise, the withdrawal requests are returned.
#[inline]
pub fn apply_withdrawal_requests_contract_call<EXT, DB: Database + DatabaseCommit>(
    evm: &mut Evm<'_, EXT, DB>,
) -> Result<Vec<Request>, BlockExecutionError>
where
    DB::Error: std::fmt::Display,
{
    // get previous env
    let previous_env = Box::new(evm.context.env().clone());

    // modify env for pre block call
    fill_tx_env_with_withdrawal_requests_contract_call(&mut evm.context.evm.env);

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

        withdrawal_requests.push(Request::WithdrawalRequest(WithdrawalRequest {
            source_address,
            validator_pubkey,
            amount,
        }));
    }

    Ok(withdrawal_requests)
}

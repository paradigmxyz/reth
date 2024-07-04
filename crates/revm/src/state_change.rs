use alloy_eips::eip2935::{HISTORY_STORAGE_ADDRESS, HISTORY_STORAGE_CODE};
use reth_chainspec::{ChainSpec, EthereumHardforks};
use reth_consensus_common::calc;
use reth_execution_errors::{BlockExecutionError, BlockValidationError};
use reth_primitives::{Address, Block, Withdrawal, Withdrawals, B256, U256};
use reth_storage_errors::provider::ProviderError;
use revm::{
    primitives::{Account, AccountInfo, Bytecode, EvmStorageSlot, BLOCKHASH_SERVE_WINDOW},
    Database, DatabaseCommit,
};

// reuse revm's hashbrown implementation for no-std
#[cfg(not(feature = "std"))]
use crate::precompile::HashMap;
#[cfg(not(feature = "std"))]
use alloc::{boxed::Box, format, string::ToString, vec::Vec};

#[cfg(feature = "std")]
use std::collections::HashMap;

/// Collect all balance changes at the end of the block.
///
/// Balance changes might include the block reward, uncle rewards, withdrawals, or irregular
/// state changes (DAO fork).
#[inline]
pub fn post_block_balance_increments(
    chain_spec: &ChainSpec,
    block: &Block,
    total_difficulty: U256,
) -> HashMap<Address, u128> {
    let mut balance_increments = HashMap::new();

    // Add block rewards if they are enabled.
    if let Some(base_block_reward) =
        calc::base_block_reward(chain_spec, block.number, block.difficulty, total_difficulty)
    {
        // Ommer rewards
        for ommer in &block.ommers {
            *balance_increments.entry(ommer.beneficiary).or_default() +=
                calc::ommer_reward(base_block_reward, block.number, ommer.number);
        }

        // Full block reward
        *balance_increments.entry(block.beneficiary).or_default() +=
            calc::block_reward(base_block_reward, block.ommers.len());
    }

    // process withdrawals
    insert_post_block_withdrawals_balance_increments(
        chain_spec,
        block.timestamp,
        block.withdrawals.as_ref().map(Withdrawals::as_ref),
        &mut balance_increments,
    );

    balance_increments
}

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
pub fn apply_blockhashes_update<DB: Database<Error: Into<ProviderError>> + DatabaseCommit>(
    db: &mut DB,
    chain_spec: &ChainSpec,
    block_timestamp: u64,
    block_number: u64,
    parent_block_hash: B256,
) -> Result<(), BlockExecutionError>
where
    DB::Error: core::fmt::Display,
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
        .map_err(|err| BlockValidationError::BlockHashAccountLoadingFailed(err.into()))?
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
fn eip2935_block_hash_slot<DB: Database<Error: Into<ProviderError>>>(
    db: &mut DB,
    block_number: u64,
    block_hash: B256,
) -> Result<(U256, EvmStorageSlot), BlockValidationError> {
    let slot = U256::from(block_number % BLOCKHASH_SERVE_WINDOW as u64);
    let current_hash = db
        .storage(HISTORY_STORAGE_ADDRESS, slot)
        .map_err(|err| BlockValidationError::BlockHashAccountLoadingFailed(err.into()))?;

    Ok((slot, EvmStorageSlot::new_changed(current_hash, block_hash.into())))
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

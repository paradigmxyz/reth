use reth_consensus_common::calc;
use reth_execution_errors::{BlockExecutionError, BlockValidationError};
use reth_primitives::{
    address, revm::env::fill_tx_env_with_beacon_root_contract_call, Address, ChainSpec, Header,
    Withdrawal, B256, U256,
};
use revm::{
    interpreter::Host,
    primitives::{Account, AccountInfo, StorageSlot},
    Database, DatabaseCommit, Evm,
};
use std::{collections::HashMap, ops::Rem};

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
pub const HISTORY_SERVE_WINDOW: usize = 8192;

/// todo: temporary move over of constants from revm until we've migrated to the latest version
pub const HISTORY_STORAGE_ADDRESS: Address = address!("25a219378dad9b3503c8268c9ca836a52427a4fb");

/// Applies the pre-block state change outlined in [EIP-2935] to store historical blockhashes in a
/// system contract.
///
/// If Prague is not activated, or the block is the genesis block, then this is a no-op, and no
/// state changes are made.
///
/// If the provided block is the fork activation block, this will generate multiple state changes,
/// as it inserts multiple historical blocks, as outlined in the EIP.
///
/// If the provided block is after Prague has been activated, this will only insert a single block
/// hash.
///
/// [EIP-2935]: https://eips.ethereum.org/EIPS/eip-2935
#[inline]
pub fn apply_blockhashes_update<DB: Database + DatabaseCommit>(
    chain_spec: &ChainSpec,
    block_timestamp: u64,
    block_number: u64,
    db: &mut DB,
) -> Result<(), BlockExecutionError>
where
    DB::Error: std::fmt::Display,
{
    // If Prague is not activated or this is the genesis block, no hashes are added.
    if !chain_spec.is_prague_active_at_timestamp(block_timestamp) || block_number == 0 {
        return Ok(())
    }
    assert!(block_number > 0);

    // Create an empty account using the `From<AccountInfo>` impl of `Account`. This marks the
    // account internally as `Loaded`, which is required, since we want the EVM to retrieve storage
    // values from the DB when `BLOCKHASH` is invoked.
    let mut account = Account::from(AccountInfo::default());

    // HACK(onbjerg): This is a temporary workaround to make sure the account does not get cleared
    // by state clearing later. This balance will likely be present in the devnet 0 genesis file
    // until the EIP itself is fixed.
    account.info.balance = U256::from(1);

    // We load the `HISTORY_STORAGE_ADDRESS` account because REVM expects this to be loaded in order
    // to access any storage, which we will do below.
    db.basic(HISTORY_STORAGE_ADDRESS)
        .map_err(|err| BlockValidationError::Eip2935StateTransition { message: err.to_string() })?;

    // Insert the state change for the slot
    let (slot, value) = eip2935_block_hash_slot(block_number - 1, db)
        .map_err(|err| BlockValidationError::Eip2935StateTransition { message: err.to_string() })?;
    account.storage.insert(slot, value);

    // If the first slot in the ring is `U256::ZERO`, then we can assume the ring has not been
    // filled before, and this is the activation block.
    //
    // Reasoning:
    // - If `block_number <= HISTORY_SERVE_WINDOW`, then the ring will be filled with as many blocks
    //   as possible, down to slot 0.
    //
    //   For example, if it is activated at block 100, then slots `0..100` will be filled.
    //
    // - If the fork is activated at genesis, then this will only run at block 1, which will fill
    //   the ring with the hash of block 0 at slot 0.
    //
    // - If the activation block is above `HISTORY_SERVE_WINDOW`, then `0..HISTORY_SERVE_WINDOW`
    //   will be filled.
    let is_activation_block = db
        .storage(HISTORY_STORAGE_ADDRESS, U256::ZERO)
        .map_err(|err| BlockValidationError::Eip2935StateTransition { message: err.to_string() })?
        .is_zero();

    // If this is the activation block, then we backfill the storage of the account with up to
    // `HISTORY_SERVE_WINDOW - 1` ancestors' blockhashes as well, per the EIP.
    //
    // Note: The -1 is because the ancestor itself was already inserted up above.
    if is_activation_block {
        let mut ancestor_block_number = block_number - 1;
        for _ in 0..HISTORY_SERVE_WINDOW - 1 {
            // Stop at genesis
            if ancestor_block_number == 0 {
                break
            }
            ancestor_block_number -= 1;

            let (slot, value) =
                eip2935_block_hash_slot(ancestor_block_number, db).map_err(|err| {
                    BlockValidationError::Eip2935StateTransition { message: err.to_string() }
                })?;
            account.storage.insert(slot, value);
        }
    }

    // Mark the account as touched and commit the state change
    account.mark_touch();
    db.commit(HashMap::from([(HISTORY_STORAGE_ADDRESS, account)]));

    Ok(())
}

/// Helper function to create a [`StorageSlot`] for [EIP-2935] state transitions for a given block
/// number.
///
/// This calculates the correct storage slot in the `BLOCKHASH` history storage address, fetches the
/// blockhash and creates a [`StorageSlot`] with appropriate previous and new values.
fn eip2935_block_hash_slot<DB: Database>(
    block_number: u64,
    db: &mut DB,
) -> Result<(U256, StorageSlot), DB::Error> {
    let slot = U256::from(block_number).rem(U256::from(HISTORY_SERVE_WINDOW));
    let current_hash = db.storage(HISTORY_STORAGE_ADDRESS, slot)?;
    let ancestor_hash = db.block_hash(U256::from(block_number))?;

    Ok((slot, StorageSlot::new_changed(current_hash, ancestor_hash.into())))
}

/// Applies the pre-block call to the [EIP-4788] beacon block root contract, using the given block,
/// [ChainSpec], EVM.
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
            for withdrawal in withdrawals.iter() {
                if withdrawal.amount > 0 {
                    *balance_increments.entry(withdrawal.address).or_default() +=
                        withdrawal.amount_wei().to::<u128>();
                }
            }
        }
    }
}

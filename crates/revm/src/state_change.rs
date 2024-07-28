use crate::precompile::HashMap;
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

#[cfg(test)]
mod tests {
    use super::*;
    use reth_ethereum_forks::{ChainHardforks, EthereumHardfork, ForkCondition};
    use reth_primitives::constants::GWEI_TO_WEI;

    /// Tests that the function correctly inserts balance increments when the Shanghai hardfork is
    /// active and there are withdrawals.
    #[test]
    fn test_insert_post_block_withdrawals_balance_increments_shanghai_active_with_withdrawals() {
        // Arrange
        // Create a ChainSpec with the Shanghai hardfork active at timestamp 100
        let chain_spec = ChainSpec {
            hardforks: ChainHardforks::new(vec![(
                Box::new(EthereumHardfork::Shanghai),
                ForkCondition::Timestamp(100),
            )]),
            ..Default::default()
        };

        // Define the block timestamp and withdrawals
        let block_timestamp = 1000;
        let withdrawals = vec![
            Withdrawal {
                address: Address::from([1; 20]),
                amount: 1000,
                index: 45,
                validator_index: 12,
            },
            Withdrawal {
                address: Address::from([2; 20]),
                amount: 500,
                index: 412,
                validator_index: 123,
            },
        ];

        // Create an empty HashMap to hold the balance increments
        let mut balance_increments = HashMap::new();

        // Act
        // Call the function with the prepared inputs
        insert_post_block_withdrawals_balance_increments(
            &chain_spec,
            block_timestamp,
            Some(&withdrawals),
            &mut balance_increments,
        );

        // Assert
        // Verify that the balance increments map has the correct number of entries
        assert_eq!(balance_increments.len(), 2);
        // Verify that the balance increments map contains the correct values for each address
        assert_eq!(
            *balance_increments.get(&Address::from([1; 20])).unwrap(),
            (1000 * GWEI_TO_WEI).into()
        );
        assert_eq!(
            *balance_increments.get(&Address::from([2; 20])).unwrap(),
            (500 * GWEI_TO_WEI).into()
        );
    }

    /// Tests that the function correctly handles the case when Shanghai is active but there are no
    /// withdrawals.
    #[test]
    fn test_insert_post_block_withdrawals_balance_increments_shanghai_active_no_withdrawals() {
        // Arrange
        // Create a ChainSpec with the Shanghai hardfork active
        let chain_spec = ChainSpec {
            hardforks: ChainHardforks::new(vec![(
                Box::new(EthereumHardfork::Shanghai),
                ForkCondition::Timestamp(100),
            )]),
            ..Default::default()
        };

        // Define the block timestamp and an empty list of withdrawals
        let block_timestamp = 1000;
        let withdrawals = Vec::<Withdrawal>::new();

        // Create an empty HashMap to hold the balance increments
        let mut balance_increments = HashMap::new();

        // Act
        // Call the function with the prepared inputs
        insert_post_block_withdrawals_balance_increments(
            &chain_spec,
            block_timestamp,
            Some(&withdrawals),
            &mut balance_increments,
        );

        // Assert
        // Verify that the balance increments map is empty
        assert!(balance_increments.is_empty());
    }

    /// Tests that the function correctly handles the case when Shanghai is not active even if there
    /// are withdrawals.
    #[test]
    fn test_insert_post_block_withdrawals_balance_increments_shanghai_not_active_with_withdrawals()
    {
        // Arrange
        // Create a ChainSpec without the Shanghai hardfork active
        let chain_spec = ChainSpec::default(); // Mock chain spec with Shanghai not active

        // Define the block timestamp and withdrawals
        let block_timestamp = 1000;
        let withdrawals = vec![
            Withdrawal {
                address: Address::from([1; 20]),
                amount: 1000,
                index: 45,
                validator_index: 12,
            },
            Withdrawal {
                address: Address::from([2; 20]),
                amount: 500,
                index: 412,
                validator_index: 123,
            },
        ];

        // Create an empty HashMap to hold the balance increments
        let mut balance_increments = HashMap::new();

        // Act
        // Call the function with the prepared inputs
        insert_post_block_withdrawals_balance_increments(
            &chain_spec,
            block_timestamp,
            Some(&withdrawals),
            &mut balance_increments,
        );

        // Assert
        // Verify that the balance increments map is empty
        assert!(balance_increments.is_empty());
    }

    /// Tests that the function correctly handles the case when Shanghai is active but all
    /// withdrawals have zero amounts.
    #[test]
    fn test_insert_post_block_withdrawals_balance_increments_shanghai_active_with_zero_withdrawals()
    {
        // Arrange
        // Create a ChainSpec with the Shanghai hardfork active
        let chain_spec = ChainSpec {
            hardforks: ChainHardforks::new(vec![(
                Box::new(EthereumHardfork::Shanghai),
                ForkCondition::Timestamp(100),
            )]),
            ..Default::default()
        };

        // Define the block timestamp and withdrawals with zero amounts
        let block_timestamp = 1000;
        let withdrawals = vec![
            Withdrawal {
                address: Address::from([1; 20]),
                amount: 0, // Zero withdrawal amount
                index: 45,
                validator_index: 12,
            },
            Withdrawal {
                address: Address::from([2; 20]),
                amount: 0, // Zero withdrawal amount
                index: 412,
                validator_index: 123,
            },
        ];

        // Create an empty HashMap to hold the balance increments
        let mut balance_increments = HashMap::new();

        // Act
        // Call the function with the prepared inputs
        insert_post_block_withdrawals_balance_increments(
            &chain_spec,
            block_timestamp,
            Some(&withdrawals),
            &mut balance_increments,
        );

        // Assert
        // Verify that the balance increments map is empty
        assert!(balance_increments.is_empty());
    }

    /// Tests that the function correctly handles the case when Shanghai is active but there are no
    /// withdrawals provided.
    #[test]
    fn test_insert_post_block_withdrawals_balance_increments_shanghai_active_with_empty_withdrawals(
    ) {
        // Arrange
        // Create a ChainSpec with the Shanghai hardfork active
        let chain_spec = ChainSpec {
            hardforks: ChainHardforks::new(vec![(
                Box::new(EthereumHardfork::Shanghai),
                ForkCondition::Timestamp(100),
            )]),
            ..Default::default()
        };

        // Define the block timestamp and no withdrawals
        let block_timestamp = 1000;
        let withdrawals = None; // No withdrawals provided

        // Create an empty HashMap to hold the balance increments
        let mut balance_increments = HashMap::new();

        // Act
        // Call the function with the prepared inputs
        insert_post_block_withdrawals_balance_increments(
            &chain_spec,
            block_timestamp,
            withdrawals,
            &mut balance_increments,
        );

        // Assert
        // Verify that the balance increments map is empty
        assert!(balance_increments.is_empty());
    }
}

use alloy_eips::eip1559::INITIAL_BASE_FEE;
use reth_chainspec::{ChainSpecBuilder, EthChainSpec};

#[test]
fn test_next_block_base_fee() {
    let chain_spec = ChainSpecBuilder::mainnet().build();

    // Test case 1: Base fee increases when gas used > target
    let parent_gas_used = 20_000_000; // More than target (15M)
    let parent_gas_limit = 30_000_000;
    let parent_base_fee = 1_000_000_000;
    let parent_timestamp = 1_700_000_000; // Recent timestamp

    let next_base_fee = chain_spec.next_block_base_fee(
        parent_gas_used,
        parent_gas_limit,
        parent_base_fee,
        parent_timestamp,
    );

    // Base fee should increase when block is more than half full
    assert!(next_base_fee > parent_base_fee);

    // Test case 2: Base fee decreases when gas used < target
    let parent_gas_used = 10_000_000; // Less than target (15M)
    let next_base_fee = chain_spec.next_block_base_fee(
        parent_gas_used,
        parent_gas_limit,
        parent_base_fee,
        parent_timestamp,
    );

    // Base fee should decrease when block is less than half full
    assert!(next_base_fee < parent_base_fee);

    // Test case 3: Base fee stays same when gas used = target
    let parent_gas_used = parent_gas_limit / 2; // Exactly at target
    let next_base_fee = chain_spec.next_block_base_fee(
        parent_gas_used,
        parent_gas_limit,
        parent_base_fee,
        parent_timestamp,
    );

    // Base fee should stay the same when block is exactly half full
    assert_eq!(next_base_fee, parent_base_fee);
}

#[test]
fn test_london_activation_base_fee() {
    let chain_spec = ChainSpecBuilder::mainnet().build();

    // At London activation, the base fee should be INITIAL_BASE_FEE
    // regardless of parent block's gas usage
    let parent_gas_used = 30_000_000;
    let parent_gas_limit = 30_000_000;
    let parent_timestamp = 1_000_000;

    // The validation logic should use INITIAL_BASE_FEE for London activation block
    // This test verifies the centralized method works with the validation logic
    let next_base_fee = chain_spec.next_block_base_fee(
        parent_gas_used,
        parent_gas_limit,
        INITIAL_BASE_FEE, // For London activation, parent base fee is set to INITIAL_BASE_FEE
        parent_timestamp,
    );

    // Should calculate normally from INITIAL_BASE_FEE
    assert!(next_base_fee > INITIAL_BASE_FEE);
}

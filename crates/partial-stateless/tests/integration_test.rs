//! Integration test: simulate multiple blocks flowing through the network cache.

use alloy_primitives::{Address, B256, U256};
use partial_stateless::{
    accessed_state::BlockAccessedState,
    network_cache::NetworkStateCache,
    policy::{AccountData, LastNBlocksPolicy},
};

/// Simulate 20 blocks with some overlapping state access patterns.
#[test]
fn test_multi_block_simulation() {
    // Account policy: keep for 10 blocks
    // Storage policy: keep for 5 blocks (more aggressive eviction)
    let mut cache = NetworkStateCache::new(
        Box::new(LastNBlocksPolicy::new(10)),
        Box::new(LastNBlocksPolicy::new(5)),
    );

    // Common "hot" addresses (like USDT, WETH) that appear in every block
    let hot_addr = Address::repeat_byte(0xAA);
    let hot_slot = B256::repeat_byte(0x01);

    // Cold addresses that appear only once
    let cold_addrs: Vec<Address> = (0..20).map(|i| Address::repeat_byte(i)).collect();

    let mut total_accessed = 0usize;
    let mut total_missed = 0usize;

    for block in 100..120 {
        let mut accessed = BlockAccessedState::default();

        // Hot address always accessed
        accessed.accounts.insert(
            hot_addr,
            AccountData { nonce: block, balance: U256::from(block * 1000), code_hash: None },
        );
        accessed.storage.insert((hot_addr, hot_slot), U256::from(block));

        // One cold address per block
        let cold = cold_addrs[block as usize - 100];
        accessed.accounts.insert(
            cold,
            AccountData { nonce: 0, balance: U256::from(100), code_hash: None },
        );

        // Compute miss before updating cache
        let miss = cache.compute_miss(&accessed);
        total_accessed += miss.total_accessed;
        total_missed += miss.total_missed;

        // Update cache with this block's data
        let stats = cache.on_block_executed(block, &accessed);

        // Verify hot address is always cached after first block
        if block > 100 {
            // Hot addr should hit cache (was inserted in previous block)
            assert!(
                cache.contains_account(&hot_addr),
                "hot address should be cached at block {block}"
            );
        }

        // Print stats for debugging
        eprintln!(
            "Block {block}: miss_ratio={:.1}%, accounts_cached={}, storage_cached={}, evicted_storage={}",
            miss.miss_ratio * 100.0,
            cache.snapshot().total_accounts,
            cache.snapshot().total_storage_slots,
            stats.storage_evicted,
        );
    }

    // After all blocks:
    let snap = cache.snapshot();

    // Account window=10: hot addr + cold addrs within window
    // Window includes current block and 10 blocks back, so up to 12 accounts at steady state
    // (hot + 11 cold addrs that are within the 10-block window due to inclusive boundary)
    assert!(snap.total_accounts <= 12, "should have at most 12 accounts, got {}", snap.total_accounts);

    // Storage window=5: hot_slot was accessed every block, so it's always retained
    assert!(cache.contains_storage(&hot_addr, &hot_slot));

    // Overall: first block is 100% miss, subsequent blocks should have lower miss
    let overall_miss_ratio = total_missed as f64 / total_accessed as f64;
    eprintln!("Overall miss ratio: {:.1}%", overall_miss_ratio * 100.0);
    // The hot address is always hit after first block, so miss < 100%
    assert!(overall_miss_ratio < 1.0);
}

/// Test that separate policies allow keeping accounts longer than storage.
#[test]
fn test_differentiated_policies() {
    let mut cache = NetworkStateCache::new(
        Box::new(LastNBlocksPolicy::new(20)), // accounts: keep 20 blocks
        Box::new(LastNBlocksPolicy::new(3)),  // storage: keep only 3 blocks
    );

    let addr = Address::repeat_byte(0x42);
    let slot = B256::repeat_byte(0x01);

    // Block 10: access account + storage
    let mut accessed = BlockAccessedState::default();
    accessed.accounts.insert(
        addr,
        AccountData { nonce: 1, balance: U256::from(500), code_hash: None },
    );
    accessed.storage.insert((addr, slot), U256::from(99));
    cache.on_block_executed(10, &accessed);

    // Block 14: storage cutoff = 11 (evicts slot accessed at 10)
    //           account cutoff = 0 (still well within window)
    cache.on_block_executed(14, &BlockAccessedState::default());

    assert!(cache.contains_account(&addr), "account should survive with window=20");
    assert!(!cache.contains_storage(&addr, &slot), "storage should be evicted with window=3");
}

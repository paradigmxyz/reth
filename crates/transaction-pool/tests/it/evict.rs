//! Transaction pool eviction tests.

use rand::distributions::Uniform;
use reth_primitives::{constants::MIN_PROTOCOL_BASE_FEE, Address, B256};
use reth_transaction_pool::{
    error::PoolErrorKind,
    test_utils::{
        MockFeeRange, MockTransactionDistribution, MockTransactionRatio, TestPool, TestPoolBuilder,
    },
    BlockInfo, PoolConfig, SubPoolLimit, TransactionOrigin, TransactionPool, TransactionPoolExt,
};

#[tokio::test(flavor = "multi_thread")]
async fn only_blobs_eviction() {
    // This test checks that blob transactions can be inserted into the pool, and at each step the
    // blob pool can be truncated to the correct size

    // set the pool limits to something small
    let pool_config = PoolConfig {
        pending_limit: SubPoolLimit { max_txs: 10, max_size: 1000 },
        queued_limit: SubPoolLimit { max_txs: 10, max_size: 1000 },
        basefee_limit: SubPoolLimit { max_txs: 10, max_size: 1000 },
        blob_limit: SubPoolLimit { max_txs: 10, max_size: 1000 },
        ..Default::default()
    };

    let pool: TestPool = TestPoolBuilder::default().with_config(pool_config.clone()).into();
    let block_info = BlockInfo {
        last_seen_block_hash: B256::ZERO,
        last_seen_block_number: 0,
        pending_basefee: 10,
        pending_blob_fee: Some(10),
    };
    pool.set_block_info(block_info);

    // this is how many times the test will regenerate transactions and insert them into the pool
    let total_txs = 1000;

    // If we have a wide size range we can cover cases both where we have a lot of small txs and a
    // lot of large txs
    let size_range = 10..1100;

    // create mock tx distribution, 100% blobs
    let tx_ratio = MockTransactionRatio {
        legacy_pct: 0,
        dynamic_fee_pct: 0,
        blob_pct: 100,
        access_list_pct: 0,
    };

    // Vary the amount of senders
    let senders = [1, 10, 100, total_txs];
    for sender_amt in &senders {
        let gas_limit_range = 100_000..1_000_000;

        // split the total txs into the amount of senders
        let txs_per_sender = total_txs / sender_amt;
        let nonce_range = 0..txs_per_sender;
        let pending_blob_fee = block_info.pending_blob_fee.unwrap();

        // start the fees at zero, some transactions will be underpriced
        let fee_range = MockFeeRange {
            gas_price: Uniform::from(0u128..(block_info.pending_basefee as u128 + 1000)),
            priority_fee: Uniform::from(0u128..(block_info.pending_basefee as u128 + 1000)),
            // we need to set the max fee to at least the min protocol base fee, or transactions
            // generated could be rejected
            max_fee: Uniform::from(
                MIN_PROTOCOL_BASE_FEE as u128..(block_info.pending_basefee as u128 + 2000),
            ),
            max_fee_blob: Uniform::from(pending_blob_fee..(pending_blob_fee + 1000)),
        };

        let distribution = MockTransactionDistribution::new(
            tx_ratio.clone(),
            fee_range,
            gas_limit_range,
            size_range.clone(),
        );

        for _ in 0..*sender_amt {
            // use a random sender, create the tx set
            let sender = Address::random();
            let set = distribution.tx_set(sender, nonce_range.clone(), &mut rand::thread_rng());

            let set = set.into_vec();

            // ensure that the first nonce is 0
            assert_eq!(set[0].get_nonce(), 0);

            // and finally insert it into the pool
            let results = pool.add_transactions(TransactionOrigin::External, set).await;
            for (i, result) in results.iter().enumerate() {
                match result {
                    Ok(hash) => {
                        println!("✅ Inserted tx into pool with hash: {hash}");
                    }
                    Err(e) => {
                        match e.kind {
                            PoolErrorKind::DiscardedOnInsert => {
                                println!("✅ Discarded tx on insert, like we should have");
                            }
                            PoolErrorKind::SpammerExceededCapacity(addr) => {
                                // ensure the address is the same as the sender
                                assert_eq!(addr, sender);

                                // ensure that this is only returned when the sender is over the
                                // pool limit per account
                                if i + 1 < pool_config.max_account_slots {
                                    panic!("Spammer exceeded capacity, but it shouldn't have. Max accounts slots: {}, current txs by sender: {}", pool_config.max_account_slots, i + 1);
                                }
                                // at this point we know that the sender has been limited, so we
                                // keep going
                            }
                            _ => {
                                panic!("Failed to insert tx into pool with unexpected error: {e}");
                            }
                        }
                    }
                }
            }

            // after every insert, ensure that it's under the pool limits
            assert!(!pool.is_exceeded());
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn mixed_eviction() {
    // This test checks that many transaction types can be inserted into the pool. The fees need
    // to be set so that the transactions will actually pass validation. Transactions here do not
    // have nonce gaps.
    let pool_config = PoolConfig {
        pending_limit: SubPoolLimit { max_txs: 20, max_size: 2000 },
        queued_limit: SubPoolLimit { max_txs: 20, max_size: 2000 },
        basefee_limit: SubPoolLimit { max_txs: 20, max_size: 2000 },
        blob_limit: SubPoolLimit { max_txs: 20, max_size: 2000 },
        ..Default::default()
    };

    let pool: TestPool = TestPoolBuilder::default().with_config(pool_config.clone()).into();
    let block_info = BlockInfo {
        last_seen_block_hash: B256::ZERO,
        last_seen_block_number: 0,
        pending_basefee: 10,
        pending_blob_fee: Some(20),
    };
    pool.set_block_info(block_info);

    let total_txs = 100;
    let size_range = 10..1100;

    // Adjust the ratios to include a mix of transaction types
    let tx_ratio = MockTransactionRatio {
        legacy_pct: 25,
        dynamic_fee_pct: 25,
        blob_pct: 25,
        access_list_pct: 25,
    };

    let senders = [1, 5, 10];
    for sender_amt in &senders {
        let gas_limit_range = 100_000..1_000_000;
        let txs_per_sender = total_txs / sender_amt;
        let nonce_range = 0..txs_per_sender;
        let pending_blob_fee = block_info.pending_blob_fee.unwrap();

        // Make sure transactions are not immediately rejected
        let min_gas_price = block_info.pending_basefee as u128 + 1;
        let min_priority_fee = 1u128;
        let min_max_fee = block_info.pending_basefee as u128 + 10;

        let fee_range = MockFeeRange {
            gas_price: Uniform::from(min_gas_price..(min_gas_price + 1000)),
            priority_fee: Uniform::from(min_priority_fee..(min_priority_fee + 1000)),
            max_fee: Uniform::from(min_max_fee..(min_max_fee + 2000)),
            max_fee_blob: Uniform::from(pending_blob_fee..(pending_blob_fee + 1000)),
        };

        let distribution = MockTransactionDistribution::new(
            tx_ratio.clone(),
            fee_range,
            gas_limit_range,
            size_range.clone(),
        );

        for _ in 0..*sender_amt {
            let sender = Address::random();
            let set = distribution.tx_set_non_conflicting_types(
                sender,
                nonce_range.clone(),
                &mut rand::thread_rng(),
            );

            let set = set.into_inner().into_vec();
            assert_eq!(set[0].get_nonce(), 0);

            let results = pool.add_transactions(TransactionOrigin::External, set).await;
            for (i, result) in results.iter().enumerate() {
                match result {
                    Ok(_) => {
                        // Transaction inserted successfully
                    }
                    Err(e) => {
                        match e.kind {
                            PoolErrorKind::DiscardedOnInsert => {
                                // Transaction discarded on insert
                                println!("✅ Discarded tx on insert, like we should have");
                            }
                            PoolErrorKind::SpammerExceededCapacity(addr) => {
                                // ensure the address is the same as the sender
                                assert_eq!(addr, sender);

                                // ensure that this is only returned when the sender is over the
                                // pool limit per account
                                if i + 1 < pool_config.max_account_slots {
                                    panic!("Spammer exceeded capacity, but it shouldn't have. Max accounts slots: {}, current txs by sender: {}", pool_config.max_account_slots, i + 1);
                                }
                            }
                            _ => panic!("Failed to insert tx into pool with unexpected error: {e}"),
                        }
                    }
                }
            }

            assert!(!pool.is_exceeded());
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn nonce_gaps_eviction() {
    // This test checks that many transaction types can be inserted into the pool.
    //
    // This test also inserts nonce gaps into the non-blob transactions.
    let pool_config = PoolConfig {
        pending_limit: SubPoolLimit { max_txs: 20, max_size: 2000 },
        queued_limit: SubPoolLimit { max_txs: 20, max_size: 2000 },
        basefee_limit: SubPoolLimit { max_txs: 20, max_size: 2000 },
        blob_limit: SubPoolLimit { max_txs: 20, max_size: 2000 },
        ..Default::default()
    };

    let pool: TestPool = TestPoolBuilder::default().with_config(pool_config.clone()).into();
    let block_info = BlockInfo {
        last_seen_block_hash: B256::ZERO,
        last_seen_block_number: 0,
        pending_basefee: 10,
        pending_blob_fee: Some(20),
    };
    pool.set_block_info(block_info);

    let total_txs = 100;
    let size_range = 10..1100;

    // Adjust the ratios to include a mix of transaction types
    let tx_ratio = MockTransactionRatio {
        legacy_pct: 25,
        dynamic_fee_pct: 25,
        blob_pct: 25,
        access_list_pct: 25,
    };

    let senders = [1, 5, 10];
    for sender_amt in &senders {
        let gas_limit_range = 100_000..1_000_000;
        let txs_per_sender = total_txs / sender_amt;
        let nonce_range = 0..txs_per_sender;
        let pending_blob_fee = block_info.pending_blob_fee.unwrap();

        // Make sure transactions are not immediately rejected
        let min_gas_price = block_info.pending_basefee as u128 + 1;
        let min_priority_fee = 1u128;
        let min_max_fee = block_info.pending_basefee as u128 + 10;

        let fee_range = MockFeeRange {
            gas_price: Uniform::from(min_gas_price..(min_gas_price + 1000)),
            priority_fee: Uniform::from(min_priority_fee..(min_priority_fee + 1000)),
            max_fee: Uniform::from(min_max_fee..(min_max_fee + 2000)),
            max_fee_blob: Uniform::from(pending_blob_fee..(pending_blob_fee + 1000)),
        };

        let distribution = MockTransactionDistribution::new(
            tx_ratio.clone(),
            fee_range,
            gas_limit_range,
            size_range.clone(),
        );

        // set up gap percentages and sizes, 30% chance for transactions to be followed by a gap,
        // and the gap size is between 1 and 5
        let gap_pct = 30;
        let gap_range = 1u64..6;

        for _ in 0..*sender_amt {
            let sender = Address::random();

            let mut set = distribution.tx_set_non_conflicting_types(
                sender,
                nonce_range.clone(),
                &mut rand::thread_rng(),
            );

            set.with_nonce_gaps(gap_pct, gap_range.clone(), &mut rand::thread_rng());
            let set = set.into_inner().into_vec();

            let results = pool.add_transactions(TransactionOrigin::External, set).await;
            for (i, result) in results.iter().enumerate() {
                match result {
                    Ok(_) => {
                        // Transaction inserted successfully
                    }
                    Err(e) => {
                        match e.kind {
                            PoolErrorKind::DiscardedOnInsert => {
                                // Transaction discarded on insert
                                println!("✅ Discarded tx on insert, like we should have");
                            }
                            PoolErrorKind::SpammerExceededCapacity(addr) => {
                                // ensure the address is the same as the sender
                                assert_eq!(addr, sender);

                                // ensure that this is only returned when the sender is over the
                                // pool limit per account
                                if i + 1 < pool_config.max_account_slots {
                                    panic!("Spammer exceeded capacity, but it shouldn't have. Max accounts slots: {}, current txs by sender: {}", pool_config.max_account_slots, i + 1);
                                }
                            }
                            _ => panic!("Failed to insert tx into pool with unexpected error: {e}"),
                        }
                    }
                }
            }

            assert!(!pool.is_exceeded());
        }
    }
}

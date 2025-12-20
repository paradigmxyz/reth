# Unique Test Transactions Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Ensure test fixtures generate unique transaction hashes per block so `TransactionHashNumbers` inserts never collide.

**Architecture:** Add a small helper to build a signed transaction with a configurable nonce, and assign a unique transaction to each test block. Add a regression test that asserts uniqueness for `BlockchainTestData::default()` and `default_from_number()`.

**Tech Stack:** Rust, reth-provider test utilities, cargo test.

### Task 1: Add a regression test for unique tx hashes in fixture data

**Files:**
- Modify: `crates/storage/provider/src/providers/database/provider.rs`

**Step 1: Write the failing test**

Add a test in the existing `tests` module near other receipts tests:

```rust
    #[test]
    fn test_blockchain_test_data_has_unique_tx_hashes() {
        let data = BlockchainTestData::default();
        let hashes: Vec<_> = data
            .blocks
            .iter()
            .flat_map(|(block, _)| block.body().transactions_iter().map(|tx| tx.tx_hash()))
            .collect();
        let mut sorted = hashes.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), hashes.len(), "duplicate tx hashes in test data");
    }
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p reth-provider providers::database::provider::tests::test_blockchain_test_data_has_unique_tx_hashes -- --nocapture`  
Expected: FAIL with "duplicate tx hashes in test data".

### Task 2: Make each test block's transaction unique

**Files:**
- Modify: `crates/storage/provider/src/test_utils/blocks.rs`

**Step 1: Write minimal implementation**

Introduce a helper and use it in each block builder:

```rust
fn test_transaction(nonce: u64) -> TransactionSigned {
    TransactionSigned::new_unhashed(
        Transaction::Legacy(TxLegacy {
            nonce,
            gas_price: 10,
            gas_limit: 400_000,
            to: TxKind::Call(hex!("095e7baea6a6c7c4c2dfeb977efac326af552d87").into()),
            ..Default::default()
        }),
        Signature::new(
            U256::from_str("51983300959770368863831494747186777928121405155922056726144551509338672451120").unwrap(),
            U256::from_str("29056683545955299640297374067888344259176096769870751649153779895496107008675").unwrap(),
            false,
        ),
    )
}
```

Then, in `block1`..`block5` after setting `body.withdrawals`, set:

```rust
    body.transactions = vec![test_transaction(number)];
```

**Step 2: Run test to verify it passes**

Run: `cargo test -p reth-provider providers::database::provider::tests::test_blockchain_test_data_has_unique_tx_hashes -- --nocapture`  
Expected: PASS.

### Task 3: Verify the original CI failures are fixed

**Files:**
- None

**Step 1: Run the two failing tests**

Run: `cargo test -p reth-provider providers::database::provider::tests::test_receipts_by_block_range_multiple_blocks -- --nocapture`  
Expected: PASS.

Run: `cargo test -p reth-provider providers::database::provider::tests::test_receipts_by_block_range_consistency_with_individual_calls -- --nocapture`  
Expected: PASS.

### Task 4: Commit

**Step 1: Commit changes**

```bash
git add crates/storage/provider/src/test_utils/blocks.rs \
  crates/storage/provider/src/providers/database/provider.rs
git commit -m "test: make test blocks use unique tx hashes"
```

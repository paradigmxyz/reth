//! BAL-path block execution — workers produce per-tx results, a canonical executor commits
//! them in order while building the composed BAL.
//!
//! This is the main-thread commit loop from `BAL.md` §Main thread — canonical executor.
//!
//! ### The `Self::Result` type-punning
//!
//! Workers and the canonical executor are both built from the same `evm_config.create_executor`,
//! but worker's Evm is over [`SnapshotDatabase`] while canonical's is over the caller's `DB`.
//! `BlockExecutor::Result` depends on `Self::Evm`, so at the type level the two sides produce
//! *distinct* `Self::Result` types — even though for `EthEvmFactory` they concretely resolve
//! to the identical `EthTxResult<HaltReason, TxType>` (because [`EvmFactory::HaltReason`] is
//! factory-level, not DB-parameterized; see `alloy-evm-0.33.1/src/evm.rs:279`).
//!
//! Rust's type system can't express that parametricity as a `where`-clause, so threading the
//! worker output into `canonical.commit_transaction(worker_result)` requires a deliberate
//! type-pun. [`reinterpret_result`] is the single `unsafe` site where this happens. It is
//! sound **only** when the `EvmFactory`'s `HaltReason` and the receipt builder's transaction
//! type are DB-independent — which holds for `EthEvmFactory` (mainnet Ethereum) and is the
//! assumption under which this function operates.
//!
//! Future work (blocks removing the unsafe):
//! - Expose `BlockExecutorFactory::Result` as an associated type in alloy-evm so it's named on the
//!   factory rather than the executor, and parameterize it only by factory-level associated types.
//!   Then Rust can prove the two sides produce the same `Result`.
//!
//! ### Checks performed
//!
//! - **A (structural hash)**: `check_bal_hash(received_bal, header_bal_hash)` at entry.
//! - **B (item-count gate)**: `check_item_count(received_bal, block_gas_limit)` at entry.
//! - **D (feasibility)**: after each tx commits, via [`FeasibilityTracker`].
//! - **F (final hash)**: rebuilt composed BAL hashed against the header's commitment after
//!   post-execution.
//!
//! Check E (per-tx fragment compare) is skipped — check F is authoritative, and a
//! lightweight fragment compare isn't yet designed.
//!
//! ### Sequential; parallelism is future work
//!
//! One worker per tx on the calling thread. Workers' isolation from the canonical side is
//! preserved (separate states, separate `bal_builder`), so a future `WorkerPool` wrap around
//! the worker path would slot in without changing the commit-loop shape.

use super::{
    feasibility::FeasibilityTracker,
    pre_state::BlockPreState,
    snapshot_db::SnapshotDatabase,
    validation::{check_bal_hash, check_item_count},
    RejectReason,
};
use alloy_eip7928::{compute_block_access_list_hash, BlockAccessList};
use alloy_evm::{
    block::{BlockExecutionError, BlockExecutor, BlockExecutorFactory, TxResult},
    Evm,
};
use alloy_primitives::B256;
use reth_evm::{execute::ExecutableTxFor, ConfigureEvm};
use reth_primitives_traits::{NodePrimitives, SealedBlock};
use revm::{
    database::{states::bundle_state::BundleRetention, BundleState, State},
    Database,
};
use revm_state::bal::Bal;
use std::sync::Arc;

/// Alias for the canonical receipt type produced by a given `ConfigureEvm`. Factory-level
/// associated type — DB-independent.
pub type ReceiptFor<Evm> =
    <<Evm as ConfigureEvm>::BlockExecutorFactory as BlockExecutorFactory>::Receipt;

/// Output of a successful BAL-path block execution.
#[expect(missing_debug_implementations)]
pub struct BalExecutionOutput<Evm: ConfigureEvm> {
    /// Accumulated state transitions from the canonical executor.
    pub bundle_state: BundleState,
    /// Receipts produced in order, one per committed tx.
    pub receipts: Vec<ReceiptFor<Evm>>,
    /// Total gas used by all transactions.
    pub gas_used: u64,
    /// Blob gas used by the block.
    pub blob_gas_used: u64,
    /// EIP-7685 withdrawal / deposit / consolidation requests.
    pub requests: alloy_eips::eip7685::Requests,
}

/// Errors surfaced by [`execute_block_bal`].
#[derive(Debug)]
pub enum BalExecutionError {
    /// BAL-specific rejection — structural, feasibility, or final-hash mismatch.
    Reject(RejectReason),
    /// Worker or canonical EVM failure (including revm `BalError` for undeclared accesses,
    /// surfaced opaquely until upstream revm carries address/slot metadata).
    Evm(BlockExecutionError),
    /// The received BAL could not be converted from alloy-format to revm-format.
    BalConversion(String),
}

impl From<RejectReason> for BalExecutionError {
    fn from(r: RejectReason) -> Self {
        Self::Reject(r)
    }
}

impl From<BlockExecutionError> for BalExecutionError {
    fn from(e: BlockExecutionError) -> Self {
        Self::Evm(e)
    }
}

impl core::fmt::Display for BalExecutionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Reject(r) => write!(f, "BAL rejection: {r:?}"),
            Self::Evm(e) => write!(f, "evm execution failed: {e}"),
            Self::BalConversion(s) => write!(f, "alloy→revm BAL conversion failed: {s}"),
        }
    }
}

impl core::error::Error for BalExecutionError {}

/// Top-level BAL-path block executor. Owns the `evm_config` so methods on `&self` give
/// revm's lifetime unification the method-scoped `&'a self` it needs.
#[expect(missing_debug_implementations)]
pub struct BalPayloadExecutor<Evm: ConfigureEvm> {
    evm_config: Evm,
}

impl<Evm: ConfigureEvm> BalPayloadExecutor<Evm> {
    /// Wraps an `evm_config`. Caller typically clones the engine's config.
    pub const fn new(evm_config: Evm) -> Self {
        Self { evm_config }
    }

    /// Executes one block on the BAL path.
    ///
    /// # Safety
    ///
    /// Contains one `unsafe` site (see module-level docs and [`reinterpret_result`]) that
    /// type-puns worker output into the canonical executor's `commit_transaction` input. The
    /// pun is sound only when the `EvmFactory`'s `HaltReason` is DB-independent, which holds
    /// for `EthEvmFactory`. Custom factories whose `HaltReason` varies by DB would produce
    /// unsound reads.
    #[expect(clippy::too_many_arguments)]
    pub fn execute_block<DB, Tx>(
        &self,
        canonical_db: DB,
        snapshot: Arc<BlockPreState>,
        received_bal: BlockAccessList,
        block: &SealedBlock<<Evm::Primitives as NodePrimitives>::Block>,
        txs: Vec<Tx>,
        header_bal_hash: B256,
        block_gas_limit: u64,
    ) -> Result<BalExecutionOutput<Evm>, BalExecutionError>
    where
        DB: Database + core::fmt::Debug,
        Tx: ExecutableTxFor<Evm>,
    {
        check_bal_hash(&received_bal, header_bal_hash)?;
        check_item_count(&received_bal, block_gas_limit)?;

        let received_bal_revm: Arc<Bal> = Arc::new(
            Bal::try_from(received_bal.clone())
                .map_err(|e| BalExecutionError::BalConversion(format!("{e:?}")))?,
        );

        let mut canonical_state = State::builder()
            .with_database(canonical_db)
            .with_bundle_update()
            .with_bal_builder()
            .build();

        // Pre-load every BAL-declared address into canonical state's cache. `State::commit`
        // (called by `commit_transaction`) panics at revm-database's
        // `cache.rs:195` ("All accounts should be present inside cache") when it tries to
        // apply a diff for an address not previously loaded. In the normal serial flow the
        // EVM loads the account itself during execution, but in option (a) the worker runs
        // the EVM on its own state and we only feed the diff to canonical — canonical never
        // read those accounts, so they aren't in its cache.
        for account_changes in &received_bal {
            canonical_state
                .load_cache_account(account_changes.address)
                .map_err(|e| BalExecutionError::Evm(BlockExecutionError::other(e)))?;
        }

        // Declare `worker_state` at the outer scope so its lifetime matches canonical_state's
        // — both are method-local, and their borrows unify at the same `'a` when
        // `executor_for_block` is called. Each iteration re-initializes with a fresh `State`.
        //
        // The initial binding is immediately overwritten in the loop but gives us a valid
        // declaration site outside the loop body so the type is fixed once.
        let mut worker_state = State::builder()
            .with_database(SnapshotDatabase::new(snapshot.clone()))
            .with_bal(received_bal_revm.clone())
            .with_bundle_update()
            .build();
        let _ = &mut worker_state; // silence unused-assignment warning — we overwrite per iter

        let block_result = {
            let mut canonical_executor = self
                .evm_config
                .executor_for_block(&mut canonical_state, block)
                .map_err(|e| BalExecutionError::Evm(BlockExecutionError::other(e)))?;

            canonical_executor.apply_pre_execution_changes()?;

            let mut feasibility = FeasibilityTracker::new(&received_bal, block_gas_limit);

            for (i, tx) in txs.into_iter().enumerate() {
                let tx_index = i as u64 + 1;

                // Reset worker state for the new tx (fresh snapshot view, zero bundle).
                worker_state = State::builder()
                    .with_database(SnapshotDatabase::new(snapshot.clone()))
                    .with_bal(received_bal_revm.clone())
                    .with_bundle_update()
                    .build();
                worker_state.set_bal_index(tx_index);

                let worker_result = {
                    let mut worker_executor = self
                        .evm_config
                        .executor_for_block(&mut worker_state, block)
                        .map_err(|e| BalExecutionError::Evm(BlockExecutionError::other(e)))?;

                    worker_executor.execute_transaction_without_commit(tx)?
                };

                feasibility.record_and_check(worker_result.result())?;

                canonical_executor.evm_mut().db_mut().bump_bal_index();
                // SAFETY: `worker_result` and `canonical_executor`'s `Self::Result` are
                // `EthTxResult<HaltReason, TxType>` for the same factory — bit-identical
                // even though the Evm types differ by DB. See module-level safety note.
                let canonical_result = unsafe { reinterpret_result(worker_result) };
                let _gas = canonical_executor.commit_transaction(canonical_result)?;
            }

            canonical_executor.evm_mut().db_mut().bump_bal_index();
            canonical_executor.apply_post_execution_changes()?
        };

        let composed_alloy = canonical_state.take_built_alloy_bal().expect("with_bal_builder set");
        let rebuilt = compute_block_access_list_hash(&composed_alloy);
        if rebuilt != header_bal_hash {
            return Err(BalExecutionError::Reject(RejectReason::FinalHashMismatch {
                rebuilt,
                expected: header_bal_hash,
            }));
        }

        canonical_state.merge_transitions(BundleRetention::Reverts);
        Ok(BalExecutionOutput {
            bundle_state: canonical_state.take_bundle(),
            receipts: block_result.receipts,
            gas_used: block_result.gas_used,
            blob_gas_used: block_result.blob_gas_used,
            requests: block_result.requests,
        })
    }
}

/// Reinterprets a worker's `Self::Result` as the canonical executor's `Self::Result`.
///
/// # Safety
///
/// Safe iff:
/// 1. The source and destination types are bit-identical.
/// 2. The source value is dropped via the destination type's `Drop` (uses `transmute_copy` +
///    `forget` to transfer ownership).
///
/// For `ConfigureEvm` implementations built atop `EthEvmFactory`, both `Self::Result` types
/// resolve to `EthTxResult<HaltReason, TxType>` regardless of DB, so (1) holds. (2) is
/// enforced by the function body. Non-Ethereum factories whose `HaltReason` depends on DB
/// would violate (1) — this function must not be called against them.
///
/// The runtime assertion is a defensive check; a size mismatch indicates that the calling
/// context does NOT satisfy (1) and the program would otherwise experience UB.
#[inline]
unsafe fn reinterpret_result<From, To>(src: From) -> To {
    assert_eq!(
        core::mem::size_of::<From>(),
        core::mem::size_of::<To>(),
        "BAL worker/canonical Self::Result types differ in size — \
         the factory's HaltReason is not DB-independent, which violates the safety invariant \
         of `execute_block_bal`. This probably means you're running against a custom \
         EvmFactory whose HaltReason varies with the database type.",
    );
    assert_eq!(
        core::mem::align_of::<From>(),
        core::mem::align_of::<To>(),
        "BAL worker/canonical Self::Result alignments differ — see size-mismatch message.",
    );
    // SAFETY: preconditions documented on the outer `unsafe fn`; size + align asserted above.
    let dst = unsafe { core::mem::transmute_copy(&src) };
    core::mem::forget(src);
    dst
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_eips::{
        eip2935::{HISTORY_STORAGE_ADDRESS, HISTORY_STORAGE_CODE},
        eip4788::{BEACON_ROOTS_ADDRESS, BEACON_ROOTS_CODE},
        eip7002::{WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS, WITHDRAWAL_REQUEST_PREDEPLOY_CODE},
    };
    use alloy_primitives::{keccak256, B256, U256};
    use reth_ethereum_primitives::{Block, BlockBody, TransactionSigned};
    use reth_evm_ethereum::EthEvmConfig;
    use reth_primitives_traits::{Block as _, Recovered, SealedBlock};
    use revm::{
        database::{CacheDB, EmptyDB},
        state::{AccountInfo, Bytecode},
    };

    /// Builds an in-memory canonical DB pre-populated with the post-Cancun system contracts
    /// that `apply_pre_execution_changes` calls: beacon roots (EIP-4788), withdrawal requests
    /// (EIP-7002), and historical block hashes (EIP-2935).
    fn system_contracts_db() -> CacheDB<EmptyDB> {
        let mut db = CacheDB::<EmptyDB>::new(Default::default());
        db.insert_account_info(
            BEACON_ROOTS_ADDRESS,
            AccountInfo {
                balance: U256::ZERO,
                nonce: 1,
                code_hash: keccak256(BEACON_ROOTS_CODE.clone()),
                code: Some(Bytecode::new_raw(BEACON_ROOTS_CODE.clone())),
                account_id: None,
            },
        );
        db.insert_account_info(
            WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS,
            AccountInfo {
                balance: U256::ZERO,
                nonce: 1,
                code_hash: keccak256(WITHDRAWAL_REQUEST_PREDEPLOY_CODE.clone()),
                code: Some(Bytecode::new_raw(WITHDRAWAL_REQUEST_PREDEPLOY_CODE.clone())),
                account_id: None,
            },
        );
        db.insert_account_info(
            HISTORY_STORAGE_ADDRESS,
            AccountInfo {
                balance: U256::ZERO,
                nonce: 1,
                code_hash: keccak256(HISTORY_STORAGE_CODE.clone()),
                code: Some(Bytecode::new_raw(HISTORY_STORAGE_CODE.clone())),
                account_id: None,
            },
        );
        db
    }

    /// Builds a minimal sealed block (empty body, Amsterdam-ready header) for tests.
    fn empty_amsterdam_block(header_bal_hash: B256) -> SealedBlock<Block> {
        let header = Header {
            timestamp: 1,
            number: 1,
            gas_limit: 30_000_000,
            parent_beacon_block_root: Some(B256::ZERO),
            withdrawals_root: Some(alloy_consensus::EMPTY_ROOT_HASH),
            requests_hash: Some(alloy_eips::eip7685::EMPTY_REQUESTS_HASH),
            excess_blob_gas: Some(0),
            blob_gas_used: Some(0),
            block_access_list_hash: Some(header_bal_hash),
            ..Header::default()
        };
        let block = Block {
            header,
            body: BlockBody {
                transactions: vec![],
                ommers: vec![],
                withdrawals: Some(vec![].into()),
            },
        };
        block.seal_slow()
    }

    #[test]
    fn rejects_bad_bal_hash_up_front() {
        // Check A: the received BAL's hash must match the header's commitment.
        let executor = BalPayloadExecutor::new(EthEvmConfig::mainnet());
        let snapshot = Arc::new(BlockPreState::default());
        let received_bal: BlockAccessList = Vec::new();
        let wrong_hash = B256::repeat_byte(0xff);
        let block = empty_amsterdam_block(wrong_hash);

        let result = executor.execute_block::<_, Recovered<TransactionSigned>>(
            system_contracts_db(),
            snapshot,
            received_bal,
            &block,
            Vec::new(),
            wrong_hash,
            30_000_000,
        );

        match result {
            Err(BalExecutionError::Reject(RejectReason::HeaderHashMismatch { .. })) => {}
            Err(e) => panic!("expected HeaderHashMismatch, got error {e:?}"),
            Ok(_) => panic!("expected HeaderHashMismatch, got Ok"),
        }
    }

    #[test]
    fn rejects_item_count_exceeding_gas_budget() {
        // Check B: (addrs + unique_slots) * 2000 must be <= gas_limit.
        use alloy_eip7928::AccountChanges;

        let executor = BalPayloadExecutor::new(EthEvmConfig::mainnet());
        let snapshot = Arc::new(BlockPreState::default());

        // 10 accounts × 2000 gas = 20,000. We set the limit to 10,000 → reject.
        let received_bal: BlockAccessList = (0u8..10)
            .map(|i| {
                let mut addr_bytes = [0u8; 20];
                addr_bytes[19] = i;
                AccountChanges { address: addr_bytes.into(), ..Default::default() }
            })
            .collect();

        let bal_hash = alloy_eip7928::compute_block_access_list_hash(&received_bal);
        let block = empty_amsterdam_block(bal_hash);

        let result = executor.execute_block::<_, Recovered<TransactionSigned>>(
            system_contracts_db(),
            snapshot,
            received_bal,
            &block,
            Vec::new(),
            bal_hash,
            10_000,
        );

        match result {
            Err(BalExecutionError::Reject(RejectReason::ItemCountExceedsGasBudget { .. })) => {}
            Err(e) => panic!("expected ItemCountExceedsGasBudget, got error {e:?}"),
            Ok(_) => panic!("expected ItemCountExceedsGasBudget, got Ok"),
        }
    }

    /// Runs only the canonical phases (pre-exec → post-exec, no txs) against a fresh
    /// `system_contracts_db()` to compute the composed BAL a block produces. Used to build
    /// the "reference" received BAL for the happy-path test below.
    ///
    /// This intentionally mirrors what `BalPayloadExecutor::execute_block` does internally,
    /// but without any hash check — the output is the BAL itself, not a pass/fail signal.
    fn reference_bal_for_empty_block(evm_config: &EthEvmConfig) -> BlockAccessList {
        use revm::database::State as RevmState;

        let db = system_contracts_db();
        let mut state =
            RevmState::builder().with_database(db).with_bundle_update().with_bal_builder().build();

        // Any header_bal_hash on the reference block is fine — we don't check it here.
        let block = empty_amsterdam_block(B256::ZERO);
        {
            let mut executor =
                evm_config.executor_for_block(&mut state, &block).expect("build executor");
            executor.apply_pre_execution_changes().expect("pre-exec");
            executor.evm_mut().db_mut().bump_bal_index();
            executor.apply_post_execution_changes().expect("post-exec");
        }
        state.take_built_alloy_bal().expect("with_bal_builder was set")
    }

    #[test]
    fn empty_block_happy_path_round_trip() {
        // Two-pass end-to-end:
        //   1. Build the canonical BAL an empty Amsterdam block produces (via
        //      `reference_bal_for_empty_block`).
        //   2. Hash it, stamp the header, and run `BalPayloadExecutor::execute_block` with that
        //      BAL. Every check must pass (A, B, D, F).
        let evm_config = EthEvmConfig::mainnet();

        let received_bal = reference_bal_for_empty_block(&evm_config);
        let bal_hash = alloy_eip7928::compute_block_access_list_hash(&received_bal);
        // Sanity: reference BAL is non-empty (system calls populated it).
        assert!(!received_bal.is_empty(), "empty BAL means system calls didn't record state");

        let executor = BalPayloadExecutor::new(evm_config);
        let snapshot = Arc::new(BlockPreState::default());
        let block = empty_amsterdam_block(bal_hash);

        let result = executor.execute_block::<_, Recovered<TransactionSigned>>(
            system_contracts_db(),
            snapshot,
            received_bal,
            &block,
            Vec::new(),
            bal_hash,
            30_000_000,
        );

        match result {
            Ok(output) => {
                assert!(output.receipts.is_empty(), "empty block → no receipts");
            }
            Err(e) => panic!("expected success, got {e:?}"),
        }
    }

    /// Inserts `AccountInfo { nonce: 0, balance }` for `addr` into the canonical DB.
    fn insert_funded(db: &mut CacheDB<EmptyDB>, addr: alloy_primitives::Address, balance: U256) {
        db.insert_account_info(
            addr,
            AccountInfo { nonce: 0, balance, code_hash: B256::ZERO, code: None, account_id: None },
        );
    }

    /// Inserts `(address → Some(Account { balance, nonce, bytecode_hash: None }))` into the
    /// snapshot's account map so BalDatabase-fallback reads resolve.
    fn seed_snapshot_account(
        pre: &mut BlockPreState,
        addr: alloy_primitives::Address,
        balance: U256,
        nonce: u64,
    ) {
        pre.accounts.insert(
            addr,
            Some(reth_primitives_traits::Account { balance, nonce, bytecode_hash: None }),
        );
    }

    /// Runs the canonical path on a block with real txs (no hash check) and returns the
    /// composed BAL. Used to build the reference BAL for happy-path multi-tx tests.
    fn reference_bal_for_block<Tx>(
        evm_config: &EthEvmConfig,
        mut db: CacheDB<EmptyDB>,
        block: &SealedBlock<Block>,
        txs: Vec<Tx>,
    ) -> BlockAccessList
    where
        Tx: ExecutableTxFor<EthEvmConfig>,
    {
        use revm::database::State as RevmState;

        let mut state = RevmState::builder()
            .with_database(&mut db)
            .with_bundle_update()
            .with_bal_builder()
            .build();

        {
            let mut executor =
                evm_config.executor_for_block(&mut state, block).expect("build executor");
            executor.apply_pre_execution_changes().expect("pre-exec");
            for (i, tx) in txs.into_iter().enumerate() {
                executor.evm_mut().db_mut().bump_bal_index();
                executor
                    .execute_transaction(tx)
                    .unwrap_or_else(|e| panic!("tx {i} failed during reference build: {e:?}"));
            }
            executor.evm_mut().db_mut().bump_bal_index();
            executor.apply_post_execution_changes().expect("post-exec");
        }
        state.take_built_alloy_bal().expect("with_bal_builder was set")
    }

    #[test]
    fn multi_tx_happy_path_round_trip() {
        // End-to-end with two value transfers from distinct senders to the same recipient.
        //
        // 1. Fund alice and bob in a fresh canonical DB + snapshot.
        // 2. Sign tx1 (alice → carol, 100 wei) and tx2 (bob → carol, 200 wei).
        // 3. Build the reference BAL by running the block through a canonical executor with
        //    `with_bal_builder`.
        // 4. Feed that BAL into `BalPayloadExecutor::execute_block` and assert 2 receipts + no
        //    rejections.
        use alloy_consensus::TxLegacy;
        use alloy_primitives::TxKind;
        use reth_chainspec::MAINNET;
        use reth_ethereum_primitives::Transaction;
        use reth_primitives_traits::crypto::secp256k1::public_key_to_address;
        use reth_testing_utils::generators::{generate_key, rng, sign_tx_with_key_pair};

        let evm_config = EthEvmConfig::mainnet();
        let coinbase = alloy_primitives::Address::ZERO;
        let carol: alloy_primitives::Address = alloy_primitives::Address::from([0xCA; 20]);
        let sender_balance = U256::from(alloy_consensus::constants::ETH_TO_WEI);

        // Generate keypairs + derive sender addresses.
        let alice_kp = generate_key(&mut rng());
        let alice = public_key_to_address(alice_kp.public_key());
        let bob_kp = generate_key(&mut rng());
        let bob = public_key_to_address(bob_kp.public_key());

        // Canonical DB: system contracts + funded senders.
        let mut canonical_db = system_contracts_db();
        insert_funded(&mut canonical_db, alice, sender_balance);
        insert_funded(&mut canonical_db, bob, sender_balance);

        // Snapshot: same accounts — workers read through BalDatabase → SnapshotDatabase.
        let mut snapshot_pre = BlockPreState::default();
        seed_snapshot_account(&mut snapshot_pre, alice, sender_balance, 0);
        seed_snapshot_account(&mut snapshot_pre, bob, sender_balance, 0);
        seed_snapshot_account(&mut snapshot_pre, carol, U256::ZERO, 0);
        seed_snapshot_account(&mut snapshot_pre, coinbase, U256::ZERO, 0);
        let snapshot = Arc::new(snapshot_pre);

        // Sign txs.
        let chain_id = MAINNET.chain.id();
        let gas_price = 1u128; // flat low price; block has no base fee in our test header.
        let tx1 = sign_tx_with_key_pair(
            alice_kp,
            Transaction::Legacy(TxLegacy {
                chain_id: Some(chain_id),
                nonce: 0,
                gas_price,
                gas_limit: 21_000,
                to: TxKind::Call(carol),
                value: U256::from(100u64),
                input: Default::default(),
            }),
        );
        let tx2 = sign_tx_with_key_pair(
            bob_kp,
            Transaction::Legacy(TxLegacy {
                chain_id: Some(chain_id),
                nonce: 0,
                gas_price,
                gas_limit: 21_000,
                to: TxKind::Call(carol),
                value: U256::from(200u64),
                input: Default::default(),
            }),
        );
        let recovered1 = Recovered::new_unchecked(tx1, alice);
        let recovered2 = Recovered::new_unchecked(tx2, bob);

        // Reference BAL: run the block canonically through a separate executor.
        let block_for_ref = empty_amsterdam_block(B256::ZERO);
        let reference_bal = reference_bal_for_block::<Recovered<TransactionSigned>>(
            &evm_config,
            {
                // Separate fresh DB for the reference run so we don't pollute canonical_db.
                let mut db = system_contracts_db();
                db.insert_account_info(
                    alice,
                    AccountInfo {
                        nonce: 0,
                        balance: sender_balance,
                        code_hash: B256::ZERO,
                        code: None,
                        account_id: None,
                    },
                );
                db.insert_account_info(
                    bob,
                    AccountInfo {
                        nonce: 0,
                        balance: sender_balance,
                        code_hash: B256::ZERO,
                        code: None,
                        account_id: None,
                    },
                );
                db
            },
            &block_for_ref,
            vec![recovered1.clone(), recovered2.clone()],
        );
        assert!(!reference_bal.is_empty(), "expected BAL entries from pre-exec + txs");

        let bal_hash = alloy_eip7928::compute_block_access_list_hash(&reference_bal);
        let block = empty_amsterdam_block(bal_hash);

        let executor = BalPayloadExecutor::new(evm_config);
        let result = executor.execute_block(
            canonical_db,
            snapshot,
            reference_bal,
            &block,
            vec![recovered1, recovered2],
            bal_hash,
            30_000_000,
        );

        match result {
            Ok(output) => {
                assert_eq!(output.receipts.len(), 2, "expected 2 receipts");
                assert!(output.gas_used >= 2 * 21_000, "expected at least 42k gas used");
            }
            Err(e) => panic!("expected success, got {e:?}"),
        }
    }

    // ============================================================================
    // Shadow-mode harness — runs a block through the serial `BasicBlockExecutor`
    // and the BAL-path `BalPayloadExecutor`, asserts byte-equal outputs.
    // ============================================================================

    /// Output of one path in a shadow run. Both serial and BAL paths produce this shape so
    /// the harness can compare field-by-field.
    #[derive(Debug)]
    struct ShadowOutput {
        bundle_state: BundleState,
        receipts: Vec<reth_ethereum_primitives::Receipt>,
        gas_used: u64,
        requests: alloy_eips::eip7685::Requests,
    }

    /// Runs the block through the serial path and captures its full output.
    ///
    /// Uses a manual state + executor (not `BasicBlockExecutor::execute_one`) so we can both
    /// (a) capture the composed BAL for the BAL-path input and (b) pull the bundle out after.
    fn run_serial_path(
        evm_config: &EthEvmConfig,
        canonical_db: CacheDB<EmptyDB>,
        block: &SealedBlock<Block>,
        txs: &[Recovered<TransactionSigned>],
    ) -> (ShadowOutput, BlockAccessList) {
        use revm::database::State as RevmState;

        let mut state = RevmState::builder()
            .with_database(canonical_db)
            .with_bundle_update()
            .with_bal_builder()
            .build();

        let block_result = {
            let mut executor =
                evm_config.executor_for_block(&mut state, block).expect("build serial executor");
            executor.apply_pre_execution_changes().expect("serial pre-exec");
            for (i, tx) in txs.iter().cloned().enumerate() {
                executor.evm_mut().db_mut().bump_bal_index();
                executor
                    .execute_transaction(tx)
                    .unwrap_or_else(|e| panic!("serial tx {i} failed: {e:?}"));
            }
            executor.evm_mut().db_mut().bump_bal_index();
            executor.apply_post_execution_changes().expect("serial post-exec")
        };

        let bal = state.take_built_alloy_bal().expect("with_bal_builder was set");
        state.merge_transitions(BundleRetention::Reverts);
        let bundle_state = state.take_bundle();

        (
            ShadowOutput {
                bundle_state,
                receipts: block_result.receipts,
                gas_used: block_result.gas_used,
                requests: block_result.requests,
            },
            bal,
        )
    }

    /// Shadow harness. Runs the block through both paths; asserts byte-equal outputs.
    fn assert_shadow_equal(
        evm_config: EthEvmConfig,
        canonical_db_template: CacheDB<EmptyDB>,
        snapshot: Arc<BlockPreState>,
        block_header_only: SealedBlock<Block>,
        txs: Vec<Recovered<TransactionSigned>>,
        gas_limit: u64,
    ) {
        // Serial run: also produces the reference BAL we'll feed to the BAL path.
        let (serial, reference_bal) =
            run_serial_path(&evm_config, canonical_db_template.clone(), &block_header_only, &txs);

        // BAL path: stamp the hash of the reference BAL onto the header.
        let bal_hash = alloy_eip7928::compute_block_access_list_hash(&reference_bal);
        let block = empty_amsterdam_block(bal_hash); // same shape, updated hash

        let executor = BalPayloadExecutor::new(evm_config);
        let bal_out = executor
            .execute_block(
                canonical_db_template,
                snapshot,
                reference_bal,
                &block,
                txs,
                bal_hash,
                gas_limit,
            )
            .unwrap_or_else(|e| panic!("BAL path failed: {e:?}"));

        // Byte-equal assertions. Any divergence surfaces the specific field that broke.
        assert_eq!(
            serial.receipts, bal_out.receipts,
            "receipts diverge between serial and BAL paths",
        );
        assert_eq!(
            serial.gas_used, bal_out.gas_used,
            "gas_used differs: serial {} vs bal {}",
            serial.gas_used, bal_out.gas_used,
        );
        assert_eq!(
            serial.requests, bal_out.requests,
            "requests (EIP-7685) diverge between serial and BAL paths",
        );
        assert_eq!(
            serial.bundle_state, bal_out.bundle_state,
            "bundle_state diverges — the canonical state transitions don't match",
        );
    }

    #[test]
    fn shadow_empty_block() {
        // System calls only — no txs. Both paths should produce identical system-call
        // side effects in their BundleState (beacon roots storage, history storage, etc.).
        assert_shadow_equal(
            EthEvmConfig::mainnet(),
            system_contracts_db(),
            Arc::new(BlockPreState::default()),
            empty_amsterdam_block(B256::ZERO),
            Vec::new(),
            30_000_000,
        );
    }

    #[test]
    fn shadow_multi_value_transfer() {
        // Two senders → same recipient. Byte-equal across paths means: worker-produced
        // diffs commit identically to a directly-executed serial path.
        use alloy_consensus::TxLegacy;
        use alloy_primitives::TxKind;
        use reth_chainspec::MAINNET;
        use reth_ethereum_primitives::Transaction;
        use reth_primitives_traits::crypto::secp256k1::public_key_to_address;
        use reth_testing_utils::generators::{generate_key, rng, sign_tx_with_key_pair};

        let evm_config = EthEvmConfig::mainnet();
        let coinbase = alloy_primitives::Address::ZERO;
        let carol: alloy_primitives::Address = alloy_primitives::Address::from([0xCA; 20]);
        let sender_balance = U256::from(alloy_consensus::constants::ETH_TO_WEI);

        let alice_kp = generate_key(&mut rng());
        let alice = public_key_to_address(alice_kp.public_key());
        let bob_kp = generate_key(&mut rng());
        let bob = public_key_to_address(bob_kp.public_key());

        let mut db = system_contracts_db();
        insert_funded(&mut db, alice, sender_balance);
        insert_funded(&mut db, bob, sender_balance);

        let mut snap = BlockPreState::default();
        seed_snapshot_account(&mut snap, alice, sender_balance, 0);
        seed_snapshot_account(&mut snap, bob, sender_balance, 0);
        seed_snapshot_account(&mut snap, carol, U256::ZERO, 0);
        seed_snapshot_account(&mut snap, coinbase, U256::ZERO, 0);

        let chain_id = MAINNET.chain.id();
        let make_tx = |kp, to, value, nonce: u64| {
            sign_tx_with_key_pair(
                kp,
                Transaction::Legacy(TxLegacy {
                    chain_id: Some(chain_id),
                    nonce,
                    gas_price: 1,
                    gas_limit: 21_000,
                    to: TxKind::Call(to),
                    value: U256::from(value),
                    input: Default::default(),
                }),
            )
        };
        let tx1 = Recovered::new_unchecked(make_tx(alice_kp, carol, 100u64, 0), alice);
        let tx2 = Recovered::new_unchecked(make_tx(bob_kp, carol, 200u64, 0), bob);

        assert_shadow_equal(
            evm_config,
            db,
            Arc::new(snap),
            empty_amsterdam_block(B256::ZERO),
            vec![tx1, tx2],
            30_000_000,
        );
    }

    #[test]
    fn shadow_tx_with_revert() {
        // A tx that reverts in a deployed contract. Both paths must produce identical receipts
        // (success = false, gas charged, state rolled back except for gas payment + nonce bump).
        //
        // Deploys `0x60006000fd` (PUSH1 0 PUSH1 0 REVERT) at `revert_contract`. Sender calls
        // it; the call reverts; fees + nonce still apply.
        use alloy_consensus::TxLegacy;
        use alloy_primitives::{Bytes, TxKind};
        use reth_chainspec::MAINNET;
        use reth_ethereum_primitives::Transaction;
        use reth_primitives_traits::crypto::secp256k1::public_key_to_address;
        use reth_testing_utils::generators::{generate_key, rng, sign_tx_with_key_pair};
        use revm::primitives::keccak256;

        let evm_config = EthEvmConfig::mainnet();
        let coinbase = alloy_primitives::Address::ZERO;
        let revert_contract: alloy_primitives::Address =
            alloy_primitives::Address::from([0xDE; 20]);
        let sender_balance = U256::from(alloy_consensus::constants::ETH_TO_WEI);

        let alice_kp = generate_key(&mut rng());
        let alice = public_key_to_address(alice_kp.public_key());

        // Deploy the revert contract bytecode.
        let revert_code: Bytes = Bytes::from_static(&[0x60, 0x00, 0x60, 0x00, 0xfd]);
        let code_hash = keccak256(&revert_code);
        let mut db = system_contracts_db();
        insert_funded(&mut db, alice, sender_balance);
        db.insert_account_info(
            revert_contract,
            AccountInfo {
                nonce: 1,
                balance: U256::ZERO,
                code_hash,
                code: Some(Bytecode::new_raw(revert_code.clone())),
                account_id: None,
            },
        );

        // Snapshot: alice + revert_contract (with its code) + coinbase.
        let mut snap = BlockPreState::default();
        seed_snapshot_account(&mut snap, alice, sender_balance, 0);
        snap.accounts.insert(
            revert_contract,
            Some(reth_primitives_traits::Account {
                balance: U256::ZERO,
                nonce: 1,
                bytecode_hash: Some(code_hash),
            }),
        );
        snap.code.insert(code_hash, Some(reth_primitives_traits::Bytecode::new_raw(revert_code)));
        seed_snapshot_account(&mut snap, coinbase, U256::ZERO, 0);

        let tx = Recovered::new_unchecked(
            sign_tx_with_key_pair(
                alice_kp,
                Transaction::Legacy(TxLegacy {
                    chain_id: Some(MAINNET.chain.id()),
                    nonce: 0,
                    gas_price: 1,
                    gas_limit: 50_000,
                    to: TxKind::Call(revert_contract),
                    value: U256::ZERO,
                    input: Default::default(),
                }),
            ),
            alice,
        );

        assert_shadow_equal(
            evm_config,
            db,
            Arc::new(snap),
            empty_amsterdam_block(B256::ZERO),
            vec![tx],
            30_000_000,
        );
    }

    #[test]
    fn shadow_tx_with_sstore() {
        // Tx calls a deployed contract that does `SSTORE(0, 0x42)`. The storage write must
        // commit identically across serial and BAL paths — this is the first scenario that
        // exercises a real storage diff, validating that our account-only pre-load path
        // (`load_cache_account` in execute_block) is sufficient even when commits include
        // storage writes.
        //
        // Bytecode: PUSH1 0x42, PUSH1 0x00, SSTORE, STOP → `0x60 0x42 0x60 0x00 0x55 0x00`.
        use alloy_consensus::TxLegacy;
        use alloy_primitives::{Bytes, TxKind};
        use reth_chainspec::MAINNET;
        use reth_ethereum_primitives::Transaction;
        use reth_primitives_traits::crypto::secp256k1::public_key_to_address;
        use reth_testing_utils::generators::{generate_key, rng, sign_tx_with_key_pair};
        use revm::primitives::keccak256;

        let evm_config = EthEvmConfig::mainnet();
        let coinbase = alloy_primitives::Address::ZERO;
        let sstore_contract: alloy_primitives::Address =
            alloy_primitives::Address::from([0x55; 20]);
        let sender_balance = U256::from(alloy_consensus::constants::ETH_TO_WEI);

        let alice_kp = generate_key(&mut rng());
        let alice = public_key_to_address(alice_kp.public_key());

        // Deploy the SSTORE contract.
        let sstore_code: Bytes = Bytes::from_static(&[0x60, 0x42, 0x60, 0x00, 0x55, 0x00]);
        let code_hash = keccak256(&sstore_code);
        let mut db = system_contracts_db();
        insert_funded(&mut db, alice, sender_balance);
        db.insert_account_info(
            sstore_contract,
            AccountInfo {
                nonce: 1,
                balance: U256::ZERO,
                code_hash,
                code: Some(Bytecode::new_raw(sstore_code.clone())),
                account_id: None,
            },
        );

        // Snapshot: alice + sstore_contract + coinbase. Workers read these via
        // BalDatabase → SnapshotDatabase fallback when their EVM loads accounts.
        let mut snap = BlockPreState::default();
        seed_snapshot_account(&mut snap, alice, sender_balance, 0);
        snap.accounts.insert(
            sstore_contract,
            Some(reth_primitives_traits::Account {
                balance: U256::ZERO,
                nonce: 1,
                bytecode_hash: Some(code_hash),
            }),
        );
        snap.code.insert(code_hash, Some(reth_primitives_traits::Bytecode::new_raw(sstore_code)));
        seed_snapshot_account(&mut snap, coinbase, U256::ZERO, 0);
        // Storage slot 0 of sstore_contract is read (SSTORE reads pre-value for the
        // gas-cost calculation under EIP-2200), so the snapshot must contain it.
        snap.storage.insert((sstore_contract, B256::ZERO), U256::ZERO);

        let tx = Recovered::new_unchecked(
            sign_tx_with_key_pair(
                alice_kp,
                Transaction::Legacy(TxLegacy {
                    chain_id: Some(MAINNET.chain.id()),
                    nonce: 0,
                    gas_price: 1,
                    gas_limit: 100_000,
                    to: TxKind::Call(sstore_contract),
                    value: U256::ZERO,
                    input: Default::default(),
                }),
            ),
            alice,
        );

        assert_shadow_equal(
            evm_config,
            db,
            Arc::new(snap),
            empty_amsterdam_block(B256::ZERO),
            vec![tx],
            30_000_000,
        );
    }

    #[test]
    fn final_hash_check_catches_missing_system_call_entries() {
        // End-to-end: empty received BAL, but canonical pre-exec runs system calls (EIP-4788
        // beacon-root, EIP-2935 blockhash, EIP-7002 withdrawal-request) that mutate state at
        // bal_index = 0 and get recorded in the canonical `bal_builder`. The composed BAL
        // therefore isn't empty — its hash differs from keccak(rlp([])), so check F fires.
        //
        // This proves the full canonical path runs (pre-exec, tx loop over zero txs,
        // post-exec) and that `take_built_alloy_bal` + `compute_block_access_list_hash` +
        // the comparison against the header are correctly wired.
        let executor = BalPayloadExecutor::new(EthEvmConfig::mainnet());
        let snapshot = Arc::new(BlockPreState::default());
        let received_bal: BlockAccessList = Vec::new();
        let empty_bal_hash = alloy_eip7928::compute_block_access_list_hash(&received_bal);
        let block = empty_amsterdam_block(empty_bal_hash);

        let result = executor.execute_block::<_, Recovered<TransactionSigned>>(
            system_contracts_db(),
            snapshot,
            received_bal,
            &block,
            Vec::new(),
            empty_bal_hash,
            30_000_000,
        );

        match result {
            Err(BalExecutionError::Reject(RejectReason::FinalHashMismatch {
                rebuilt,
                expected,
            })) => {
                assert_eq!(expected, empty_bal_hash);
                assert_ne!(rebuilt, empty_bal_hash, "rebuilt BAL must be non-empty");
            }
            Err(e) => panic!("expected FinalHashMismatch, got error {e:?}"),
            Ok(_) => panic!("expected FinalHashMismatch, got Ok"),
        }
    }
}

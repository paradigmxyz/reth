//! BAL executor.
//!
//! Read `execute_block` as two execution paths over the same parent state.
//!
//! Worker states run transactions speculatively. Each worker gets a fresh database from
//! `make_db`, installs the received BAL, sets its transaction BAL index, and returns an
//! uncommitted transaction result.
//!
//! The canonical state owns block effects. It runs the normal pre/post block hooks, commits
//! worker results in transaction order, tracks block gas admission, and builds the BAL that this
//! execution actually produced.
//!
//! The final hash check compares that rebuilt BAL with the header commitment. The outer payload
//! validator still handles consensus checks, receipt-root validation, state-root work, and block
//! insertion.

use crate::tree::ExecutionEnv;

use super::BalExecutionError;
use alloy_evm::block::{BlockExecutionError, BlockExecutor, BlockValidationError};
use reth_evm::{
    block::BlockExecutorFactory, execute::ExecutableTxFor, ConfigureEvm, Database, ExecutionCtxFor,
};
use reth_primitives_traits::ReceiptTy;
use reth_provider::BlockExecutionOutput;
use reth_tasks::Runtime;
use revm::{
    context::{result::ResultAndState, Block},
    database::{states::bundle_state::BundleRetention, State},
    primitives::eip7825::TX_GAS_LIMIT_CAP,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// Executes one block on the BAL path using the runtime's persistent BAL worker pool.
pub fn execute_block<'a, Evm, Tx, DB, MakeDb>(
    runtime: &Runtime,
    evm_config: &'a Evm,
    make_db: MakeDb,
    ctx: ExecutionCtxFor<'a, Evm>,
    env: ExecutionEnv<Evm>,
    txs: Vec<Tx>,
) -> Result<BlockExecutionOutput<ReceiptTy<Evm::Primitives>>, BalExecutionError>
where
    Evm: ConfigureEvm + 'static,
    Tx: ExecutableTxFor<Evm> + Send,
    DB: Database + Send,
    MakeDb: Fn() -> Result<DB, BalExecutionError> + Sync,
{
    let worker_pool = runtime.bal_streaming_pool();
    let mut canonical_state =
        State::builder().with_database(make_db()?).with_bundle_update().build();

    // Pre-load every BAL-declared address into canonical state's cache. `State::commit`
    // (called by `commit_transaction`) panics at revm-database's
    // `cache.rs:195` ("All accounts should be present inside cache") when it tries to
    // apply a diff for an address not previously loaded. In the normal serial flow the
    // EVM loads the account itself during execution, but here workers execute the tx EVM
    // and the canonical loop only commits their outputs, so canonical may never have read
    // those accounts itself.
    for account_changes in env.decoded_bal.as_ref().expect("BAL is required").as_bal() {
        canonical_state
            .load_cache_account(account_changes.address)
            .map_err(|e| BalExecutionError::Evm(BlockExecutionError::other(e)))?;
    }

    let block_result = {
        let evm = evm_config.evm_with_env(&mut canonical_state, env.evm_env.clone());
        let mut canonical_executor =
            evm_config.block_executor_factory().create_executor(evm, ctx.clone());

        canonical_executor.apply_pre_execution_changes()?;

        let abort = Arc::new(AtomicBool::new(false));
        let tx_count = txs.len() as u64;
        let make_db = &make_db;
        let ctx = &ctx;
        let env = &env;

        worker_pool.in_place_scope(|scope| -> Result<(), BalExecutionError> {
            let mut result_rxs = Vec::with_capacity(tx_count as usize);

            for (i, tx) in txs.into_iter().enumerate() {
                let abort = Arc::clone(&abort);
                let result_rx = spawn_worker(scope, abort, move || {
                    let mut worker_state =
                        State::builder().with_database(make_db()?).with_bundle_update().build();

                    let evm = evm_config.evm_with_env(&mut worker_state, env.evm_env.clone());
                    evm_config
                        .block_executor_factory()
                        .create_executor(evm, ctx.clone())
                        .execute_transaction_with_index(tx, i)
                        .map_err(BalExecutionError::Evm)
                });
                result_rxs.push(result_rx);
            }
            for result_rx in result_rxs.into_iter() {
                let worker_result = result_rx.recv().map_err(|_| {
                    BalExecutionError::Evm(BlockExecutionError::msg(
                        "BAL worker result channel closed before result arrived",
                    ))
                })?;
                let worker_result = match worker_result {
                    Ok(worker_result) => worker_result,
                    Err(err) => {
                        abort.store(true, Ordering::Relaxed);
                        return Err(err);
                    }
                };
                canonical_executor.commit_transaction(worker_result);
            }

            Ok(())
        })?;

        canonical_executor.apply_post_execution_changes()?
    };

    canonical_state.merge_transitions(BundleRetention::Reverts);

    Ok(BlockExecutionOutput { result: block_result, state: canonical_state.take_bundle() })
}

/// Mirrors `EthBlockExecutor`'s cumulative gas admission check in the ordered BAL commit loop.
#[derive(Debug)]
struct BlockGasTracker {
    block_gas_limit: u64,
    is_amsterdam: bool,
    cumulative_tx_gas_used: u64,
    block_regular_gas_used: u64,
}

impl BlockGasTracker {
    const fn new(block_gas_limit: u64, is_amsterdam: bool) -> Self {
        Self { block_gas_limit, is_amsterdam, cumulative_tx_gas_used: 0, block_regular_gas_used: 0 }
    }

    fn validate_tx_limit(&self, tx_gas_limit: u64) -> Result<(), BlockExecutionError> {
        let block_gas_used = if self.is_amsterdam {
            self.block_regular_gas_used
        } else {
            self.cumulative_tx_gas_used
        };
        let block_available_gas = self.block_gas_limit.saturating_sub(block_gas_used);
        let tx_min_gas_limit = tx_gas_limit.min(TX_GAS_LIMIT_CAP);

        if tx_min_gas_limit > block_available_gas {
            return Err(BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                transaction_gas_limit: tx_gas_limit,
                block_available_gas,
            }
            .into());
        }

        Ok(())
    }

    fn record_result<H>(&mut self, result: &ResultAndState<H>) {
        let gas = result.result.gas();
        self.cumulative_tx_gas_used = self.cumulative_tx_gas_used.saturating_add(gas.tx_gas_used());
        self.block_regular_gas_used =
            self.block_regular_gas_used.saturating_add(gas.block_regular_gas_used());
    }
}

fn spawn_worker<'scope, R, F>(
    scope: &rayon::Scope<'scope>,
    abort: Arc<AtomicBool>,
    run: F,
) -> crossbeam_channel::Receiver<Result<R, BalExecutionError>>
where
    R: Send + 'scope,
    F: FnOnce() -> Result<R, BalExecutionError> + Send + 'scope,
{
    let (result_tx, result_rx) = crossbeam_channel::bounded(1);

    scope.spawn(move |_| {
        if abort.load(Ordering::Relaxed) {
            return;
        }

        let _ = result_tx.send(run());
    });

    result_rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_eip7928::{bal::Bal as AlloyBal, BlockAccessList};
    use alloy_eips::{
        eip2935::{HISTORY_STORAGE_ADDRESS, HISTORY_STORAGE_CODE},
        eip4788::{BEACON_ROOTS_ADDRESS, BEACON_ROOTS_CODE},
        eip7002::{WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS, WITHDRAWAL_REQUEST_PREDEPLOY_CODE},
    };
    use alloy_primitives::{keccak256, B256, U256};
    use reth_ethereum_primitives::{Block, BlockBody, TransactionSigned};
    use reth_evm_ethereum::EthEvmConfig;
    use reth_primitives_traits::{Block as _, Recovered, SealedBlock};
    use reth_tasks::Runtime;
    use revm::{
        database::{CacheDB, EmptyDB},
        state::{AccountInfo, Bytecode},
    };

    /// Wraps a `BlockAccessList` into an `Arc<DecodedBal>` by RLP-encoding the BAL.
    fn to_arc_decoded(bal: BlockAccessList) -> Arc<DecodedBal> {
        let alloy_bal: AlloyBal = bal.into();
        let raw = alloy_rlp::encode(&alloy_bal).into();
        Arc::new(DecodedBal::new(alloy_bal, raw))
    }

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
        empty_amsterdam_block_with_gas_limit(header_bal_hash, 30_000_000)
    }

    fn empty_amsterdam_block_with_gas_limit(
        header_bal_hash: B256,
        gas_limit: u64,
    ) -> SealedBlock<Block> {
        let header = Header {
            timestamp: 1,
            number: 1,
            gas_limit,
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

    /// Runs only the canonical phases (pre-exec → post-exec, no txs) against a fresh
    /// `system_contracts_db()` to compute the composed BAL a block produces. Used to build
    /// the "reference" received BAL for the happy-path test below.
    ///
    /// This intentionally mirrors what `execute_block` does internally,
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
        //   2. Hash it, stamp the header, and run `execute_block` with that BAL. Every check must
        //      pass (A, B, D, F).
        let evm_config = EthEvmConfig::mainnet();

        let received_bal = reference_bal_for_empty_block(&evm_config);
        let bal_hash = alloy_eip7928::compute_block_access_list_hash(&received_bal);
        // Sanity: reference BAL is non-empty (system calls populated it).
        assert!(!received_bal.is_empty(), "empty BAL means system calls didn't record state");

        let block = empty_amsterdam_block(bal_hash);

        let result = execute_block(
            &Runtime::test(),
            evm_config,
            db_factory(system_contracts_db()),
            to_arc_decoded(received_bal),
            &block,
            Vec::<Recovered<TransactionSigned>>::new(),
            bal_hash,
        );

        match result {
            Ok(output) => {
                assert!(output.receipts.is_empty(), "empty block → no receipts");
            }
            Err(e) => panic!("expected success, got {e:?}"),
        }
    }

    fn db_factory(
        db: CacheDB<EmptyDB>,
    ) -> impl Fn() -> Result<CacheDB<EmptyDB>, BalExecutionError> + Sync {
        move || Ok(db.clone())
    }

    /// Inserts `AccountInfo { nonce: 0, balance }` for `addr` into the canonical DB.
    fn insert_funded(db: &mut CacheDB<EmptyDB>, addr: alloy_primitives::Address, balance: U256) {
        db.insert_account_info(
            addr,
            AccountInfo { nonce: 0, balance, code_hash: B256::ZERO, code: None, account_id: None },
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
        // 1. Fund alice and bob in a fresh canonical DB.
        // 2. Sign tx1 (alice → carol, 100 wei) and tx2 (bob → carol, 200 wei).
        // 3. Build the reference BAL by running the block through a canonical executor with
        //    `with_bal_builder`.
        // 4. Feed that BAL into `execute_block` and assert 2 receipts + no rejections.
        use alloy_consensus::TxLegacy;
        use alloy_primitives::TxKind;
        use reth_chainspec::MAINNET;
        use reth_ethereum_primitives::Transaction;
        use reth_primitives_traits::crypto::secp256k1::public_key_to_address;
        use reth_testing_utils::generators::{generate_key, rng, sign_tx_with_key_pair};

        let evm_config = EthEvmConfig::mainnet();
        let carol: alloy_primitives::Address = alloy_primitives::Address::from([0xCA; 20]);
        let sender_balance = U256::from(alloy_consensus::constants::ETH_TO_WEI);

        // Generate keypairs + derive sender addresses.
        let alice_kp = generate_key(&mut rng());
        let alice = public_key_to_address(alice_kp.public_key());
        let bob_kp = generate_key(&mut rng());
        let bob = public_key_to_address(bob_kp.public_key());

        // Pre-block DB: system contracts + funded senders.
        let mut pre_block_db = system_contracts_db();
        insert_funded(&mut pre_block_db, alice, sender_balance);
        insert_funded(&mut pre_block_db, bob, sender_balance);

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

        let result = execute_block(
            &Runtime::test(),
            evm_config,
            db_factory(pre_block_db),
            to_arc_decoded(reference_bal),
            &block,
            vec![recovered1, recovered2],
            bal_hash,
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
    // and the BAL path, asserts byte-equal outputs.
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
        block_header_only: SealedBlock<Block>,
        txs: Vec<Recovered<TransactionSigned>>,
    ) {
        // Serial run: also produces the reference BAL we'll feed to the BAL path.
        let (serial, reference_bal) =
            run_serial_path(&evm_config, canonical_db_template.clone(), &block_header_only, &txs);

        // BAL path: stamp the hash of the reference BAL onto the header.
        let bal_hash = alloy_eip7928::compute_block_access_list_hash(&reference_bal);
        let block =
            empty_amsterdam_block_with_gas_limit(bal_hash, block_header_only.header().gas_limit());

        let bal_out = execute_block(
            &Runtime::test(),
            evm_config,
            db_factory(canonical_db_template),
            to_arc_decoded(reference_bal),
            &block,
            txs,
            bal_hash,
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
            empty_amsterdam_block(B256::ZERO),
            Vec::new(),
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
        let carol: alloy_primitives::Address = alloy_primitives::Address::from([0xCA; 20]);
        let sender_balance = U256::from(alloy_consensus::constants::ETH_TO_WEI);

        let alice_kp = generate_key(&mut rng());
        let alice = public_key_to_address(alice_kp.public_key());
        let bob_kp = generate_key(&mut rng());
        let bob = public_key_to_address(bob_kp.public_key());

        let mut db = system_contracts_db();
        insert_funded(&mut db, alice, sender_balance);
        insert_funded(&mut db, bob, sender_balance);

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

        assert_shadow_equal(evm_config, db, empty_amsterdam_block(B256::ZERO), vec![tx1, tx2]);
    }

    #[test]
    fn rejects_tx_gas_limit_that_exceeds_remaining_block_gas() {
        // Each worker sees an empty block, so both transactions fit individually. The ordered
        // commit loop must still reject tx2 because tx1's committed gas leaves too little
        // block gas for tx2's gas limit.
        use alloy_consensus::TxLegacy;
        use alloy_evm::block::BlockValidationError;
        use alloy_primitives::TxKind;
        use reth_chainspec::MAINNET;
        use reth_ethereum_primitives::Transaction;
        use reth_primitives_traits::crypto::secp256k1::public_key_to_address;
        use reth_testing_utils::generators::{generate_key, rng, sign_tx_with_key_pair};

        let evm_config = EthEvmConfig::mainnet();
        let carol: alloy_primitives::Address = alloy_primitives::Address::from([0xCA; 20]);
        let sender_balance = U256::from(alloy_consensus::constants::ETH_TO_WEI);
        let block_gas_limit = 1_000_000;
        let tx_gas_limit = 990_000;

        let alice_kp = generate_key(&mut rng());
        let alice = public_key_to_address(alice_kp.public_key());
        let bob_kp = generate_key(&mut rng());
        let bob = public_key_to_address(bob_kp.public_key());

        let mut pre_block_db = system_contracts_db();
        insert_funded(&mut pre_block_db, alice, sender_balance);
        insert_funded(&mut pre_block_db, bob, sender_balance);

        let chain_id = MAINNET.chain.id();
        let make_tx = |kp, value| {
            sign_tx_with_key_pair(
                kp,
                Transaction::Legacy(TxLegacy {
                    chain_id: Some(chain_id),
                    nonce: 0,
                    gas_price: 1,
                    gas_limit: tx_gas_limit,
                    to: TxKind::Call(carol),
                    value: U256::from(value),
                    input: Default::default(),
                }),
            )
        };
        let tx1 = Recovered::new_unchecked(make_tx(alice_kp, 100u64), alice);
        let tx2 = Recovered::new_unchecked(make_tx(bob_kp, 200u64), bob);

        // Build the reference BAL under a generous gas limit so both workers can execute.
        // Replaying the same BAL under `block_gas_limit` below should reject in the ordered
        // commit loop before tx2 is committed.
        let reference_block = empty_amsterdam_block(B256::ZERO);
        let reference_bal = reference_bal_for_block(
            &evm_config,
            pre_block_db.clone(),
            &reference_block,
            vec![tx1.clone(), tx2.clone()],
        );
        let bal_hash = alloy_eip7928::compute_block_access_list_hash(&reference_bal);
        let low_gas_block = empty_amsterdam_block_with_gas_limit(bal_hash, block_gas_limit);

        let result = execute_block(
            &Runtime::test(),
            evm_config,
            db_factory(pre_block_db),
            to_arc_decoded(reference_bal),
            &low_gas_block,
            vec![tx1, tx2],
            bal_hash,
        );

        match result {
            Err(BalExecutionError::Evm(err)) => assert!(matches!(
                err.as_validation(),
                Some(BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas { .. })
            )),
            Err(err) => panic!("expected block gas validation error, got {err:?}"),
            Ok(_) => panic!("expected block gas validation error, got Ok"),
        }
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
                code: Some(Bytecode::new_raw(revert_code)),
                account_id: None,
            },
        );

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

        assert_shadow_equal(evm_config, db, empty_amsterdam_block(B256::ZERO), vec![tx]);
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
                code: Some(Bytecode::new_raw(sstore_code)),
                account_id: None,
            },
        );

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

        assert_shadow_equal(evm_config, db, empty_amsterdam_block(B256::ZERO), vec![tx]);
    }
}

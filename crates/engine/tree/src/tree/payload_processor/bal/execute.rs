//! BAL executor.
//!
//! Read `execute_block` as two execution paths over the same parent state.
//!
//! Worker states run transactions speculatively. Each worker gets one fresh cache-filling database
//! from `make_db(true)`, installs the received BAL, sets the transaction BAL index for each
//! streamed transaction, and returns uncommitted transaction results.
//!
//! The canonical state owns block effects. It runs the normal pre/post block hooks, commits
//! worker results in transaction order, tracks block gas admission, and builds the BAL that this
//! execution actually produced.
//!
//! The rebuilt BAL is returned to the outer payload validator for consensus post-execution
//! validation. This module only logs the first divergence between the received BAL and the BAL
//! rebuilt from canonical execution.

use super::{ordered_outputs::ordered_worker_outputs, worker, BalExecutionError};
use alloy_eip7928::{
    bal::{Bal as AlloyBal, DecodedBal},
    compute_block_access_list_hash, BlockAccessList,
};
use alloy_evm::{
    block::{BlockExecutionError, BlockExecutor, BlockValidationError, TxResult},
    Evm,
};
use alloy_primitives::Address;
use crossbeam_channel::{Receiver, Sender};
use reth_evm::{execute::ExecutableTxFor, ConfigureEvm, Database, EvmEnvFor, ExecutionCtxFor};
use reth_primitives_traits::ReceiptTy;
use reth_provider::BlockExecutionOutput;
use reth_tasks::Runtime;
use revm::{
    context::{result::ResultAndState, Block},
    database::{states::bundle_state::BundleRetention, State},
    state::bal::Bal as RevmBal,
};
use std::sync::Arc;

use crate::tree::payload_processor::receipt_root_task::IndexedReceipt;

/// Executes one block on the BAL path using the runtime's persistent BAL worker pool.
#[expect(clippy::too_many_arguments, clippy::type_complexity)]
pub fn execute_block<'a, Evm, Tx, Err, DB, MakeDb>(
    runtime: &Runtime,
    evm_config: &'a Evm,
    make_db: &'a MakeDb,
    input_bal: Arc<DecodedBal>,
    evm_env: EvmEnvFor<Evm>,
    ctx: ExecutionCtxFor<'a, Evm>,
    transaction_count: usize,
    txs: Receiver<(usize, Result<Tx, Err>)>,
    receipt_tx: Sender<IndexedReceipt<ReceiptTy<Evm::Primitives>>>,
) -> Result<
    (BlockExecutionOutput<ReceiptTy<Evm::Primitives>>, Vec<Address>, BlockAccessList),
    BalExecutionError,
>
where
    Evm: ConfigureEvm + 'static,
    Tx: ExecutableTxFor<Evm> + Send + 'a,
    Err: core::error::Error + Send + Sync + 'static,
    DB: Database + Send + 'a,
    MakeDb: Fn(bool) -> Result<DB, BalExecutionError> + Sync + 'a,
    ReceiptTy<Evm::Primitives>: Clone,
{
    let worker_pool = runtime.bal_streaming_pool();
    let worker_count = worker_pool.current_num_threads().max(1).min(transaction_count);

    worker_pool.in_place_scope(|scope| {
        execute_block_inner(
            scope,
            evm_config,
            make_db,
            input_bal,
            evm_env,
            ctx,
            transaction_count,
            txs,
            receipt_tx,
            worker_count,
        )
    })
}

#[expect(clippy::too_many_arguments, clippy::type_complexity)]
fn execute_block_inner<'scope, Evm, Tx, Err, DB, MakeDb>(
    scope: &rayon::Scope<'scope>,
    evm_config: &'scope Evm,
    make_db: &'scope MakeDb,
    input_bal: Arc<DecodedBal>,
    evm_env: EvmEnvFor<Evm>,
    ctx: ExecutionCtxFor<'scope, Evm>,
    transaction_count: usize,
    txs: Receiver<(usize, Result<Tx, Err>)>,
    receipt_tx: Sender<IndexedReceipt<ReceiptTy<Evm::Primitives>>>,
    worker_count: usize,
) -> Result<
    (BlockExecutionOutput<ReceiptTy<Evm::Primitives>>, Vec<Address>, BlockAccessList),
    BalExecutionError,
>
where
    Evm: ConfigureEvm + 'scope,
    Tx: ExecutableTxFor<Evm> + Send + 'scope,
    Err: core::error::Error + Send + Sync + 'static,
    DB: Database + Send + 'scope,
    MakeDb: Fn(bool) -> Result<DB, BalExecutionError> + Sync + 'scope,
    ReceiptTy<Evm::Primitives>: Clone,
{
    let bal = input_bal.as_bal();
    let input_bal_revm = convert_alloy_to_revm_bal(bal)?;

    let block_gas_limit = evm_env.block_env.gas_limit();
    let enable_amsterdam_eip8037 = evm_env.cfg_env.enable_amsterdam_eip8037;
    let tx_gas_limit_cap = evm_env.cfg_env.tx_gas_limit_cap;
    let mut canonical_state = State::builder()
        .with_database(make_db(false)?)
        .with_bundle_update()
        .with_bal_builder()
        .build();

    let (block_result, senders) = {
        let (result_tx, result_rx) = crossbeam_channel::unbounded();
        let (abort_guard, abort_rx) = AbortGuard::new();

        for _ in 0..worker_count {
            worker::spawn_worker(
                scope,
                txs.clone(),
                abort_rx.clone(),
                result_tx.clone(),
                evm_config,
                make_db,
                Arc::clone(&input_bal_revm),
                evm_env.clone(),
                ctx.clone(),
            );
        }
        drop(result_tx);

        let mut gas_tracker =
            BlockGasTracker::new(block_gas_limit, enable_amsterdam_eip8037, tx_gas_limit_cap);
        let evm = evm_config.evm_with_env(&mut canonical_state, evm_env);
        let mut canonical_executor = evm_config.create_executor_with_state(evm, ctx.clone());

        canonical_executor.apply_pre_execution_changes()?;
        let mut senders = Vec::with_capacity(transaction_count);
        let mut last_sent_len = 0usize;
        for output in ordered_worker_outputs(&result_rx, transaction_count) {
            let output = output?;

            gas_tracker.validate_tx_limit(output.tx_gas_limit)?;
            gas_tracker.record_result(output.result.result());
            canonical_executor.evm_mut().db_mut().bump_bal_index();

            let _ = canonical_executor.commit_transaction(output.result);
            senders.push(output.signer);

            let current_len = canonical_executor.receipts().len();
            if current_len > last_sent_len {
                last_sent_len = current_len;
                if let Some(receipt) = canonical_executor.receipts().last() {
                    let tx_index = current_len - 1;
                    let _ = receipt_tx.send(IndexedReceipt::new(tx_index, receipt.clone()));
                }
            }
        }
        drop(abort_guard);

        canonical_executor.evm_mut().db_mut().bump_bal_index();
        let block_result = canonical_executor.apply_post_execution_changes()?;
        (block_result, senders)
    };

    let built_bal = take_built_bal_and_log_divergence(&mut canonical_state, bal);

    canonical_state.merge_transitions(BundleRetention::Reverts);
    Ok((
        BlockExecutionOutput { state: canonical_state.take_bundle(), result: block_result },
        senders,
        built_bal,
    ))
}

fn convert_alloy_to_revm_bal(alloy_bal: &AlloyBal) -> Result<Arc<RevmBal>, BalExecutionError> {
    // Convert the BAL from alloy to a BAL that can be consumed by revm, that is more amenable
    // for state lookups.
    //
    // This is failable.
    //
    // This is due to bytecodes. A transaction can attempt to deploy illegal bytecodes, e.g. due to
    // EIP-3541 or more specifically due to EIP-7702.
    //
    // During serial execution this check happens before the bytecode is deployed and if the check
    // is triggered then the execution is reverted, and as such no actual code change event takes
    // place. Therefore, if we do observe such a bytecode in a BAL then that means the BAL is
    // invalid as no legal execution should've led to this bytecode deployment.
    let received_bal_revm = RevmBal::clone_from_alloy(alloy_bal.as_vec()).map_err(|e| {
        BalExecutionError::Consensus(reth_consensus::ConsensusError::BlockAccessListInvalid(
            format!("{e:?}"),
        ))
    })?;
    Ok(Arc::new(received_bal_revm))
}

fn take_built_bal_and_log_divergence<DB>(
    canonical_state: &mut State<DB>,
    received_bal: &AlloyBal,
) -> BlockAccessList
where
    DB: Database,
{
    let built_bal = canonical_state.take_built_alloy_bal().expect("with_bal_builder set");
    if tracing::enabled!(target: "engine::tree::payload_processor::bal", tracing::Level::DEBUG) &&
        built_bal.as_slice() != received_bal.as_slice()
    {
        let rebuilt = compute_block_access_list_hash(built_bal.as_slice());
        let expected = compute_block_access_list_hash(received_bal.as_slice());
        let div = received_bal.diff(built_bal.as_slice());
        tracing::debug!(
            target: "engine::tree::payload_processor::bal",
            %rebuilt,
            %expected,
            %div,
            "first BAL divergence",
        );
    }

    built_bal
}

/// Closes the abort channel on drop, waking scoped workers before the scope exits.
struct AbortGuard {
    _tx: Sender<()>,
}

impl AbortGuard {
    fn new() -> (Self, Receiver<()>) {
        let (tx, rx) = crossbeam_channel::bounded(0);
        (Self { _tx: tx }, rx)
    }
}

/// Mirrors `EthBlockExecutor`'s cumulative gas admission check in the ordered BAL commit loop.
#[derive(Debug)]
struct BlockGasTracker {
    block_gas_limit: u64,
    enable_amsterdam_eip8037: bool,
    tx_gas_limit_cap: Option<u64>,
    cumulative_tx_gas_used: u64,
    block_regular_gas_used: u64,
}

impl BlockGasTracker {
    const fn new(
        block_gas_limit: u64,
        enable_amsterdam_eip8037: bool,
        tx_gas_limit_cap: Option<u64>,
    ) -> Self {
        Self {
            block_gas_limit,
            enable_amsterdam_eip8037,
            tx_gas_limit_cap,
            cumulative_tx_gas_used: 0,
            block_regular_gas_used: 0,
        }
    }

    fn validate_tx_limit(&self, tx_gas_limit: u64) -> Result<(), BlockExecutionError> {
        let block_gas_used = if self.enable_amsterdam_eip8037 {
            self.block_regular_gas_used
        } else {
            self.cumulative_tx_gas_used
        };
        let block_available_gas = self.block_gas_limit.saturating_sub(block_gas_used);
        let tx_min_gas_limit =
            self.tx_gas_limit_cap.map_or(tx_gas_limit, |cap| tx_gas_limit.min(cap));

        if tx_min_gas_limit > block_available_gas {
            return Err(BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                transaction_gas_limit: tx_gas_limit,
                block_available_gas,
            }
            .into());
        }

        Ok(())
    }

    const fn record_result<H>(&mut self, result: &ResultAndState<H>) {
        let gas = result.result.gas();
        self.cumulative_tx_gas_used = self.cumulative_tx_gas_used.saturating_add(gas.tx_gas_used());
        self.block_regular_gas_used =
            self.block_regular_gas_used.saturating_add(gas.block_regular_gas_used());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{BlockHeader, Header};
    use alloy_eip7928::{bal::Bal as AlloyBal, BlockAccessList};
    use alloy_eips::{
        eip2935::{HISTORY_STORAGE_ADDRESS, HISTORY_STORAGE_CODE},
        eip4788::{BEACON_ROOTS_ADDRESS, BEACON_ROOTS_CODE},
        eip7002::{WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS, WITHDRAWAL_REQUEST_PREDEPLOY_CODE},
    };
    use alloy_primitives::{keccak256, B256, U256};
    use reth_ethereum_primitives::{Block, BlockBody, Receipt, TransactionSigned};
    use reth_evm_ethereum::EthEvmConfig;
    use reth_primitives_traits::{Block as _, Recovered, SealedBlock};
    use reth_revm::db::BundleState;
    use reth_tasks::Runtime;
    use revm::{
        database::{CacheDB, EmptyDB},
        state::{AccountInfo, Bytecode},
    };
    use std::convert::Infallible;

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

        let input_bal = reference_bal_for_empty_block(&evm_config);
        let bal_hash = alloy_eip7928::compute_block_access_list_hash(&input_bal);
        // Sanity: reference BAL is non-empty (system calls populated it).
        assert!(!input_bal.is_empty(), "empty BAL means system calls didn't record state");

        let block = empty_amsterdam_block(bal_hash);

        let result = run_execute_block(
            &Runtime::test(),
            evm_config,
            db_factory(system_contracts_db()),
            to_arc_decoded(input_bal),
            &block,
            Vec::<Recovered<TransactionSigned>>::new(),
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

    fn tx_stream<Tx>(txs: Vec<Tx>) -> Receiver<(usize, Result<Tx, Infallible>)> {
        let (tx, rx) = crossbeam_channel::unbounded();
        for (index, transaction) in txs.into_iter().enumerate() {
            tx.send((index, Ok(transaction))).unwrap();
        }
        rx
    }

    fn run_execute_block<Tx, DB, MakeDb>(
        runtime: &Runtime,
        evm_config: EthEvmConfig,
        make_db: MakeDb,
        input_bal: Arc<DecodedBal>,
        block: &SealedBlock<Block>,
        txs: Vec<Tx>,
    ) -> Result<BlockExecutionOutput<Receipt>, BalExecutionError>
    where
        Tx: ExecutableTxFor<EthEvmConfig> + Send,
        DB: Database + Send,
        MakeDb: Fn() -> Result<DB, BalExecutionError> + Sync,
    {
        run_execute_block_full(runtime, evm_config, make_db, input_bal, block, txs)
            .map(|(output, _)| output)
    }

    fn run_execute_block_full<Tx, DB, MakeDb>(
        runtime: &Runtime,
        evm_config: EthEvmConfig,
        make_db: MakeDb,
        input_bal: Arc<DecodedBal>,
        block: &SealedBlock<Block>,
        txs: Vec<Tx>,
    ) -> Result<(BlockExecutionOutput<Receipt>, BlockAccessList), BalExecutionError>
    where
        Tx: ExecutableTxFor<EthEvmConfig> + Send,
        DB: Database + Send,
        MakeDb: Fn() -> Result<DB, BalExecutionError> + Sync,
    {
        let transaction_count = txs.len();
        let (receipt_tx, _receipt_rx) = crossbeam_channel::unbounded();
        let evm_env = evm_config.evm_env(block.header()).unwrap();
        let execution_ctx = evm_config.context_for_block(block).unwrap();
        let make_db = |_: bool| make_db();
        execute_block(
            runtime,
            &evm_config,
            &make_db,
            input_bal,
            evm_env,
            execution_ctx,
            transaction_count,
            tx_stream(txs),
            receipt_tx,
        )
        .map(|(output, _, built_bal)| (output, built_bal))
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

        let result = run_execute_block(
            &Runtime::test(),
            evm_config,
            db_factory(pre_block_db),
            to_arc_decoded(reference_bal),
            &block,
            vec![recovered1, recovered2],
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

        let bal_out = run_execute_block(
            &Runtime::test(),
            evm_config,
            db_factory(canonical_db_template),
            to_arc_decoded(reference_bal),
            &block,
            txs,
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
            serial.bundle_state, bal_out.state,
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

        let result = run_execute_block(
            &Runtime::test(),
            evm_config,
            db_factory(pre_block_db),
            to_arc_decoded(reference_bal),
            &low_gas_block,
            vec![tx1, tx2],
        );

        match result {
            Err(BalExecutionError::Execution(err)) => assert!(matches!(
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
        use alloy_primitives::{keccak256, Bytes, TxKind};
        use reth_chainspec::MAINNET;
        use reth_ethereum_primitives::Transaction;
        use reth_primitives_traits::crypto::secp256k1::public_key_to_address;
        use reth_testing_utils::generators::{generate_key, rng, sign_tx_with_key_pair};

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
        // commit identically across serial and BAL paths even though the canonical state applies
        // a diff produced by a worker EVM.
        //
        // Bytecode: PUSH1 0x42, PUSH1 0x00, SSTORE, STOP → `0x60 0x42 0x60 0x00 0x55 0x00`.
        use alloy_consensus::TxLegacy;
        use alloy_primitives::{keccak256, Bytes, TxKind};
        use reth_chainspec::MAINNET;
        use reth_ethereum_primitives::Transaction;
        use reth_primitives_traits::crypto::secp256k1::public_key_to_address;
        use reth_testing_utils::generators::{generate_key, rng, sign_tx_with_key_pair};

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

    #[test]
    fn returns_built_bal_for_final_hash_mismatch() {
        // Build the BAL an empty block actually produces, then append a phantom address
        // that execution never touches. The rebuilt BAL omits it, and the outer consensus
        // validator is responsible for comparing that rebuilt hash to the header commitment.
        use alloy_eip7928::AccountChanges;

        let evm_config = EthEvmConfig::mainnet();

        // Real BAL the block would produce.
        let real_bal = reference_bal_for_empty_block(&evm_config);
        assert!(!real_bal.is_empty(), "reference BAL must be non-empty");

        // Tamper: append a phantom address not accessed during execution.
        let phantom = alloy_primitives::Address::from([0xFF; 20]);
        let mut tampered_entries: Vec<AccountChanges> = real_bal;
        tampered_entries.push(AccountChanges::new(phantom));
        let tampered_bal: alloy_eip7928::bal::Bal = alloy_eip7928::bal::Bal::new(tampered_entries);

        // Stamp the tampered BAL's hash on the block header.
        let tampered_block_access_list: BlockAccessList = tampered_bal.clone().into();
        let tampered_hash =
            alloy_eip7928::compute_block_access_list_hash(&tampered_block_access_list);
        let block = empty_amsterdam_block(tampered_hash);

        let received = {
            let raw = alloy_rlp::encode(&tampered_bal).into();
            Arc::new(DecodedBal::new(tampered_bal, raw))
        };

        let result = run_execute_block_full(
            &Runtime::test(),
            evm_config,
            db_factory(system_contracts_db()),
            received,
            &block,
            Vec::<Recovered<TransactionSigned>>::new(),
        );

        match result {
            Ok((_, built_bal)) => {
                let rebuilt = alloy_eip7928::compute_block_access_list_hash(&built_bal);
                assert_ne!(rebuilt, tampered_hash, "rebuilt and header hashes must differ");
            }
            Err(e) => panic!("expected success with rebuilt BAL, got {e:?}"),
        }
    }

    #[test]
    fn canonical_make_db_failure() {
        // A make_db that always fails must surface as Provider before any workers are
        // spawned or the BAL is processed.
        let evm_config = EthEvmConfig::mainnet();
        let block = empty_amsterdam_block(B256::ZERO);

        let failing_make_db = || -> Result<CacheDB<EmptyDB>, BalExecutionError> {
            Err(reth_provider::ProviderError::BestBlockNotFound.into())
        };

        let result = run_execute_block(
            &Runtime::test(),
            evm_config,
            failing_make_db,
            to_arc_decoded(BlockAccessList::default()),
            &block,
            Vec::<Recovered<TransactionSigned>>::new(),
        );

        assert!(
            matches!(result, Err(BalExecutionError::Provider(_))),
            "expected Provider error from canonical make_db failure, got {result:?}",
        );
    }

    #[test]
    fn worker_tx_recovery_error_becomes_other_error() {
        // A tx recovery failure fed into the worker channel must surface as
        // BalExecutionError::Other. Uses execute_block directly since tx_stream hardcodes
        // Infallible and cannot inject errors.
        let evm_config = EthEvmConfig::mainnet();
        let block = empty_amsterdam_block(B256::ZERO);

        let (tx_tx, tx_rx) = crossbeam_channel::unbounded::<(
            usize,
            Result<Recovered<TransactionSigned>, std::io::Error>,
        )>();
        tx_tx.send((0, Err(std::io::Error::other("sig fail")))).unwrap();
        drop(tx_tx);

        let (receipt_tx, _receipt_rx) = crossbeam_channel::unbounded();
        let evm_env = evm_config.evm_env(block.header()).unwrap();
        let execution_ctx = evm_config.context_for_block(&block).unwrap();
        let make_db = db_factory(system_contracts_db());
        let make_db = |_: bool| make_db();

        let result = execute_block(
            &Runtime::test(),
            &evm_config,
            &make_db,
            to_arc_decoded(BlockAccessList::default()),
            evm_env,
            execution_ctx,
            1, // transaction_count = 1 → exactly one worker spawned
            tx_rx,
            receipt_tx,
        );

        assert!(
            matches!(result, Err(BalExecutionError::Other(_))),
            "expected Other error from tx recovery failure, got {result:?}",
        );
    }

    #[test]
    fn gas_tracker_non_amsterdam_uses_cumulative_gas() {
        // All-state-gas results keep block_regular_gas_used at 0, so a second tx that fits
        // within the block limit but not the remaining cumulative budget proves that
        // non-Amsterdam reads cumulative_tx_gas_used while Amsterdam does not.
        use revm::{
            context::result::{
                ExecResultAndState, ExecutionResult, Output, ResultGas, SuccessReason,
            },
            state::EvmState,
        };

        let block_gas_limit = 1_000_000u64;
        let first_tx_gas = 600_000u64;
        let second_tx_gas_limit = 500_000u64; // fits in total limit but not after cumulative deduction

        let gas = ResultGas::new_with_state_gas(first_tx_gas, 0, 0, first_tx_gas);
        let fake_result: ResultAndState<revm::context::result::HaltReason> =
            ExecResultAndState::new(
                ExecutionResult::Success {
                    reason: SuccessReason::Return,
                    gas,
                    logs: vec![],
                    output: Output::Call(Default::default()),
                },
                EvmState::default(),
            );

        // Non-Amsterdam: block_available_gas = 1_000_000 - 600_000 = 400_000 → reject 500_000.
        let mut non_amsterdam = BlockGasTracker::new(block_gas_limit, false, None);
        non_amsterdam.record_result(&fake_result);
        assert!(
            non_amsterdam.validate_tx_limit(second_tx_gas_limit).is_err(),
            "non-Amsterdam tracker must reject tx that exceeds remaining cumulative gas",
        );

        // Amsterdam: block_available_gas = 1_000_000 - 0 = 1_000_000 → accept 500_000.
        let mut amsterdam = BlockGasTracker::new(block_gas_limit, true, None);
        amsterdam.record_result(&fake_result);
        assert!(
            amsterdam.validate_tx_limit(second_tx_gas_limit).is_ok(),
            "Amsterdam tracker must accept the same tx since block_regular_gas_used stays 0",
        );
    }

    #[test]
    fn gas_tracker_caps_oversized_tx_gas_limit_at_tx_gas_limit_cap() {
        // A tx with gas_limit above TX_GAS_LIMIT_CAP (EIP-7825) is admitted when the
        // capped value fits in the remaining block gas and rejected when it does not.
        use revm::{
            context::result::{
                ExecResultAndState, ExecutionResult, Output, ResultGas, SuccessReason,
            },
            primitives::eip7825::TX_GAS_LIMIT_CAP,
            state::EvmState,
        };

        let block_gas_limit = 30_000_000u64;
        let oversized = TX_GAS_LIMIT_CAP + 1_000_000; // 17_777_216 — above the cap

        // Case 1: fresh block, no prior gas consumed.
        // tx_min_gas_limit = TX_GAS_LIMIT_CAP (16_777_216) ≤ block_available_gas (30M) → Ok.
        let tracker = BlockGasTracker::new(block_gas_limit, false, Some(TX_GAS_LIMIT_CAP));
        assert!(
            tracker.validate_tx_limit(oversized).is_ok(),
            "oversized tx must pass when capped limit fits in block gas",
        );

        // Case 2: prior tx consumed 20M, leaving 10M available.
        // tx_min_gas_limit = TX_GAS_LIMIT_CAP (16_777_216) > block_available_gas (10M) → Err.
        let prior_gas = 20_000_000u64;
        let gas = ResultGas::new_with_state_gas(prior_gas, 0, 0, prior_gas);
        let fake_result: ResultAndState<revm::context::result::HaltReason> =
            ExecResultAndState::new(
                ExecutionResult::Success {
                    reason: SuccessReason::Return,
                    gas,
                    logs: vec![],
                    output: Output::Call(Default::default()),
                },
                EvmState::default(),
            );

        let mut tracker = BlockGasTracker::new(block_gas_limit, false, Some(TX_GAS_LIMIT_CAP));
        tracker.record_result(&fake_result);
        assert!(
            tracker.validate_tx_limit(oversized).is_err(),
            "oversized tx must be rejected when capped limit exceeds remaining block gas",
        );
    }
}

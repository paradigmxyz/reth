//! BAL executor.
//!
//! Read `execute_block` as two execution paths over the same parent state.
//!
//! Worker states run transactions speculatively. Each worker gets one fresh database from
//! `make_db`, installs the received BAL, sets the transaction BAL index for each streamed
//! transaction, and returns uncommitted transaction results.
//!
//! The canonical state owns block effects. It runs the normal pre/post block hooks, commits
//! worker results in transaction order, tracks block gas admission, and builds the BAL that this
//! execution actually produced.
//!
//! The final hash check compares that rebuilt BAL with the header commitment. The outer payload
//! validator still handles consensus checks, receipt-root validation, state-root work, and block
//! insertion.

use super::{
    debug, ordered_outputs::ordered_worker_outputs, worker, BalExecutionError, RejectReason,
};
use alloy_consensus::BlockHeader;
use alloy_eip7928::{
    bal::{Bal as AlloyBal, DecodedBal},
    compute_block_access_list_hash,
};
use alloy_evm::{
    block::{BlockExecutionError, BlockExecutor, BlockValidationError, TxResult},
    Evm,
};
use alloy_primitives::{Address, B256};
use crossbeam_channel::{Receiver, Sender};
use reth_evm::{execute::ExecutableTxFor, ConfigureEvm, Database};
use reth_primitives_traits::{BlockTy, ReceiptTy, SealedBlock};
use reth_tasks::Runtime;
use revm::{
    context::result::ResultAndState,
    database::{states::bundle_state::BundleRetention, BundleState, State},
    primitives::{eip7825::TX_GAS_LIMIT_CAP, hardfork::SpecId},
};
use revm_state::bal::Bal as RevmBal;
use std::sync::Arc;

use crate::tree::payload_processor::receipt_root_task::IndexedReceipt;

/// Output of a successful BAL-path block execution.
#[expect(missing_debug_implementations)]
pub struct BalExecutionOutput<Evm: ConfigureEvm> {
    /// Accumulated state transitions from the canonical executor.
    pub bundle_state: BundleState,
    /// Receipts produced in order, one per committed tx.
    pub receipts: Vec<ReceiptTy<Evm::Primitives>>,
    /// Total gas used by all transactions.
    pub gas_used: u64,
    /// Blob gas used by the block.
    pub blob_gas_used: u64,
    /// EIP-7685 withdrawal / deposit / consolidation requests.
    pub requests: alloy_eips::eip7685::Requests,
}

/// Executes one block on the BAL path using the runtime's persistent BAL worker pool.
#[expect(clippy::too_many_arguments)]
pub fn execute_block<Evm, Tx, Err, DB, MakeDb>(
    runtime: &Runtime,
    evm_config: Evm,
    make_db: MakeDb,
    received_bal: Arc<DecodedBal>,
    block: &SealedBlock<BlockTy<Evm::Primitives>>,
    transaction_count: usize,
    txs: Receiver<(usize, Result<Tx, Err>)>,
    receipt_tx: Sender<IndexedReceipt<ReceiptTy<Evm::Primitives>>>,
    header_bal_hash: B256,
) -> Result<(BalExecutionOutput<Evm>, Vec<Address>), BalExecutionError>
where
    Evm: ConfigureEvm,
    Tx: ExecutableTxFor<Evm> + Send,
    Err: core::error::Error + Send + Sync + 'static,
    DB: Database + Send,
    MakeDb: Fn() -> Result<DB, BalExecutionError> + Sync,
    ReceiptTy<Evm::Primitives>: Clone,
{
    let worker_pool = runtime.bal_streaming_pool();
    let worker_count = worker_pool.current_num_threads().max(1).min(transaction_count);

    worker_pool.in_place_scope(|scope| {
        execute_block_inner(
            scope,
            evm_config,
            &make_db,
            received_bal,
            block,
            transaction_count,
            txs,
            receipt_tx,
            header_bal_hash,
            worker_count,
        )
    })
}

#[expect(clippy::too_many_arguments)]
fn execute_block_inner<'scope, Evm, Tx, Err, DB, MakeDb>(
    scope: &rayon::Scope<'scope>,
    evm_config: Evm,
    make_db: &'scope MakeDb,
    received_bal: Arc<DecodedBal>,
    block: &'scope SealedBlock<BlockTy<Evm::Primitives>>,
    transaction_count: usize,
    txs: Receiver<(usize, Result<Tx, Err>)>,
    receipt_tx: Sender<IndexedReceipt<ReceiptTy<Evm::Primitives>>>,
    header_bal_hash: B256,
    worker_count: usize,
) -> Result<(BalExecutionOutput<Evm>, Vec<Address>), BalExecutionError>
where
    Evm: ConfigureEvm + 'scope,
    Tx: ExecutableTxFor<Evm> + Send + 'scope,
    Err: core::error::Error + Send + Sync + 'static,
    DB: Database + Send + 'scope,
    MakeDb: Fn() -> Result<DB, BalExecutionError> + Sync + 'scope,
    ReceiptTy<Evm::Primitives>: Clone,
{
    let bal = received_bal.as_bal();
    let received_bal_revm: Arc<RevmBal> = Arc::new(
        RevmBal::try_from(Vec::<_>::from(bal.clone()))
            .map_err(|e| BalExecutionError::BalConversion(format!("{e:?}")))?,
    );

    // NOTE: technically Amsterdam implies BAL (the current path) we are on.
    // TODO: should we do this
    let is_amsterdam = evm_config
        .evm_env(block.header())
        .map_err(|e| BalExecutionError::Evm(BlockExecutionError::other(e)))?
        .cfg_env
        .spec
        .into()
        .is_enabled_in(SpecId::AMSTERDAM);
    let block_gas_limit = block.header().gas_limit();
    let mut canonical_state =
        State::builder().with_database(make_db()?).with_bundle_update().with_bal_builder().build();
    load_bal_accounts(&mut canonical_state, bal)?;

    let (block_result, senders) = {
        let worker_evm_config = evm_config.clone();
        let (result_tx, result_rx) = crossbeam_channel::unbounded();
        let (abort_guard, abort_rx) = AbortGuard::new();

        for _ in 0..worker_count {
            worker::spawn_worker(
                scope,
                txs.clone(),
                abort_rx.clone(),
                result_tx.clone(),
                worker_evm_config.clone(),
                make_db,
                Arc::clone(&received_bal_revm),
                block,
            );
        }
        drop(result_tx);

        let mut gas_tracker = BlockGasTracker::new(block_gas_limit, is_amsterdam);
        let mut canonical_executor = evm_config
            .executor_for_block(&mut canonical_state, block)
            .map_err(|e| BalExecutionError::Evm(BlockExecutionError::other(e)))?;

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

    validate_bal_hash(&mut canonical_state, bal, header_bal_hash)?;

    canonical_state.merge_transitions(BundleRetention::Reverts);
    Ok((
        BalExecutionOutput {
            bundle_state: canonical_state.take_bundle(),
            receipts: block_result.receipts,
            gas_used: block_result.gas_used,
            blob_gas_used: block_result.blob_gas_used,
            requests: block_result.requests,
        },
        senders,
    ))
}

fn load_bal_accounts<DB>(
    canonical_state: &mut State<DB>,
    bal: &AlloyBal,
) -> Result<(), BalExecutionError>
where
    DB: Database,
{
    // Pre-load every BAL-declared address into canonical state's cache. `State::commit`
    // (called by `commit_transaction`) panics at revm-database's
    // `cache.rs:195` ("All accounts should be present inside cache") when it tries to
    // apply a diff for an address not previously loaded. In the normal serial flow the
    // EVM loads the account itself during execution, but here workers execute the tx EVM
    // and the canonical loop only commits their outputs, so canonical may never have read
    // those accounts itself.
    for account_changes in bal {
        canonical_state
            .load_cache_account(account_changes.address)
            .map_err(|e| BalExecutionError::Evm(BlockExecutionError::other(e)))?;
    }

    Ok(())
}

fn validate_bal_hash<DB>(
    canonical_state: &mut State<DB>,
    received_bal: &AlloyBal,
    header_bal_hash: B256,
) -> Result<(), BalExecutionError>
where
    DB: Database,
{
    let composed_alloy = canonical_state.take_built_alloy_bal().expect("with_bal_builder set");
    let rebuilt = compute_block_access_list_hash(&composed_alloy);
    if rebuilt == header_bal_hash {
        return Ok(());
    }

    if tracing::enabled!(
        target: "engine::tree::payload_processor::bal",
        tracing::Level::DEBUG
    ) {
        let div = debug::first_bal_divergence(received_bal, &composed_alloy);
        tracing::debug!(
            target: "engine::tree::payload_processor::bal",
            %rebuilt,
            expected = %header_bal_hash,
            ?div,
            "first BAL divergence",
        );
    }

    Err(BalExecutionError::Reject(RejectReason::FinalHashMismatch {
        rebuilt,
        expected: header_bal_hash,
    }))
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

        let received_bal = reference_bal_for_empty_block(&evm_config);
        let bal_hash = alloy_eip7928::compute_block_access_list_hash(&received_bal);
        // Sanity: reference BAL is non-empty (system calls populated it).
        assert!(!received_bal.is_empty(), "empty BAL means system calls didn't record state");

        let block = empty_amsterdam_block(bal_hash);

        let result = run_execute_block(
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
        received_bal: Arc<DecodedBal>,
        block: &SealedBlock<Block>,
        txs: Vec<Tx>,
        bal_hash: B256,
    ) -> Result<BalExecutionOutput<EthEvmConfig>, BalExecutionError>
    where
        Tx: ExecutableTxFor<EthEvmConfig> + Send,
        DB: Database + Send,
        MakeDb: Fn() -> Result<DB, BalExecutionError> + Sync,
    {
        let transaction_count = txs.len();
        let (receipt_tx, _receipt_rx) = crossbeam_channel::unbounded();
        execute_block(
            runtime,
            evm_config,
            make_db,
            received_bal,
            block,
            transaction_count,
            tx_stream(txs),
            receipt_tx,
            bal_hash,
        )
        .map(|(output, _)| output)
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

        let bal_out = run_execute_block(
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

        let result = run_execute_block(
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

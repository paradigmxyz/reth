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
    bal::{Bal, DecodedBal},
    compute_block_access_list_hash, BlockAccessList,
};
use alloy_primitives::Address;
use crossbeam_channel::{Receiver, Sender};
use reth_evm::{
    BlockExecutionOutput, BlockExecutor, BlockExecutorFactory, BlockExecutorFor, ConfigureEvm,
    Database, EvmEnvFor, ExecutableTxFor, ExecutionCtxFor,
};
use reth_primitives_traits::ReceiptTy;
use reth_tasks::Runtime;
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
    let received_bal = Arc::new(
        <BlockExecutorFor<'scope, Evm> as BlockExecutor>::convert_block_access_list(
            input_bal.as_bal().as_vec(),
        )
        .map_err(|err| reth_errors::ConsensusError::BlockAccessListInvalid(err.to_string()))?,
    );
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
            Arc::clone(&received_bal),
            evm_env.clone(),
            ctx.clone(),
        );
    }
    drop(result_tx);

    let evm = evm_config.evm_with_env(make_db(false)?, evm_env);
    let mut canonical_executor =
        evm_config.block_executor_factory().create_executor(evm, ctx.clone());
    canonical_executor.enable_block_access_list_builder();
    canonical_executor.apply_pre_execution_changes()?;

    let mut senders = Vec::with_capacity(transaction_count);
    let mut last_sent_len = 0;
    for output in ordered_worker_outputs(&result_rx, transaction_count) {
        let output = output?;
        canonical_executor.commit_transaction(output.result)?;
        senders.push(output.signer);

        let current_len = canonical_executor.receipts().len();
        if current_len > last_sent_len {
            last_sent_len = current_len;
            if let Some(receipt) = canonical_executor.receipts().last() {
                let _ = receipt_tx.send(IndexedReceipt::new(current_len - 1, receipt.clone()));
            }
        }
    }
    drop(abort_guard);

    let (output, built_bal) = canonical_executor.finish_with_block_access_list()?;
    let built_bal = built_bal.expect("BAL builder was enabled for parallel execution");
    log_bal_divergence(&built_bal, input_bal.as_bal());
    Ok((output, senders, built_bal))
}

fn log_bal_divergence(built_bal: &BlockAccessList, received_bal: &Bal) {
    if tracing::enabled!(target: "engine::tree::payload_processor::bal", tracing::Level::DEBUG) &&
        built_bal.as_slice() != received_bal.as_slice()
    {
        let rebuilt = compute_block_access_list_hash(built_bal);
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{BlockHeader, Header, TxLegacy};
    use alloy_eip7928::{bal::Bal as AlloyBal, BlockAccessList};
    use alloy_eips::{
        eip2935::{HISTORY_STORAGE_ADDRESS, HISTORY_STORAGE_CODE},
        eip4788::{BEACON_ROOTS_ADDRESS, BEACON_ROOTS_CODE},
        eip7002::{WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS, WITHDRAWAL_REQUEST_PREDEPLOY_CODE},
    };
    use alloy_primitives::{Bytes, B256, U256};
    use reth_chainspec::{ChainSpecBuilder, MAINNET};
    use reth_ethereum_primitives::{Block, BlockBody, Receipt, Transaction, TransactionSigned};
    use reth_evm::{
        cached::{AccountInfo, Bytecode},
        BlockExecutionOutput, BlockValidationError,
    };
    use reth_evm_ethereum::EthEvmConfig;
    use reth_primitives_traits::{
        crypto::secp256k1::public_key_to_address, Block as _, Recovered, SealedBlock,
    };
    use reth_testing_utils::generators::{generate_key, rng, sign_tx_with_key_pair};
    use std::{collections::BTreeMap, convert::Infallible};

    #[derive(Clone, Default)]
    struct TestDatabase {
        accounts: BTreeMap<Address, AccountInfo>,
        contracts: BTreeMap<B256, Bytecode>,
        storage: BTreeMap<(Address, U256), U256>,
    }

    impl TestDatabase {
        fn insert_account_info(&mut self, address: &Address, info: AccountInfo) {
            if let Some(code) = info.code.as_ref() {
                self.contracts.insert(info.code_hash, code.clone());
            }
            self.accounts.insert(*address, info);
        }
    }

    impl Database for TestDatabase {
        type Error = Infallible;

        fn get_account(&mut self, address: &Address) -> Result<Option<AccountInfo>, Self::Error> {
            Ok(self.accounts.get(address).cloned())
        }

        fn get_code_by_hash(&mut self, code_hash: &B256) -> Result<Bytecode, Self::Error> {
            Ok(self.contracts.get(code_hash).cloned().unwrap_or_default())
        }

        fn get_storage(&mut self, address: &Address, key: &U256) -> Result<U256, Self::Error> {
            Ok(self.storage.get(&(*address, *key)).copied().unwrap_or_default())
        }

        fn get_block_hash(&mut self, _number: &U256) -> Result<Option<B256>, Self::Error> {
            Ok(None)
        }
    }

    fn test_evm_config() -> EthEvmConfig {
        EthEvmConfig::new(Arc::new(ChainSpecBuilder::mainnet().amsterdam_activated().build()))
    }

    fn non_amsterdam_evm_config() -> EthEvmConfig {
        EthEvmConfig::new(Arc::new(ChainSpecBuilder::mainnet().osaka_activated().build()))
    }

    fn to_arc_decoded(bal: BlockAccessList) -> Arc<DecodedBal> {
        let alloy_bal: AlloyBal = bal.into();
        let raw = alloy_rlp::encode(&alloy_bal).into();
        Arc::new(DecodedBal::new(alloy_bal, raw))
    }

    fn system_contracts_db() -> TestDatabase {
        let mut db = TestDatabase::default();
        db.insert_account_info(
            &BEACON_ROOTS_ADDRESS,
            AccountInfo::default()
                .with_nonce(1)
                .with_code(Bytecode::new_raw(BEACON_ROOTS_CODE.clone())),
        );
        db.insert_account_info(
            &WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS,
            AccountInfo::default()
                .with_nonce(1)
                .with_code(Bytecode::new_raw(WITHDRAWAL_REQUEST_PREDEPLOY_CODE.clone())),
        );
        db.insert_account_info(
            &HISTORY_STORAGE_ADDRESS,
            AccountInfo::default()
                .with_nonce(1)
                .with_code(Bytecode::new_raw(HISTORY_STORAGE_CODE.clone())),
        );
        db
    }

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
            slot_number: Some(0),
            ..Header::default()
        };
        Block {
            header,
            body: BlockBody {
                transactions: vec![],
                ommers: vec![],
                withdrawals: Some(vec![].into()),
            },
        }
        .seal_slow()
    }

    fn db_factory(
        db: TestDatabase,
    ) -> impl Fn(bool) -> Result<TestDatabase, BalExecutionError> + Sync {
        move |_| Ok(db.clone())
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
        MakeDb: Fn(bool) -> Result<DB, BalExecutionError> + Sync,
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
        MakeDb: Fn(bool) -> Result<DB, BalExecutionError> + Sync,
    {
        let transaction_count = txs.len();
        let (receipt_tx, _receipt_rx) = crossbeam_channel::unbounded();
        let evm_env = evm_config.evm_env(block.header()).unwrap();
        let execution_ctx = evm_config.context_for_block(block).unwrap();
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

    fn insert_funded(db: &mut TestDatabase, address: Address, balance: U256) {
        db.insert_account_info(&address, AccountInfo::default().with_balance(balance));
    }

    fn run_serial_path(
        evm_config: &EthEvmConfig,
        canonical_db: TestDatabase,
        block: &SealedBlock<Block>,
        txs: &[Recovered<TransactionSigned>],
    ) -> (BlockExecutionOutput<Receipt>, BlockAccessList) {
        let mut executor =
            evm_config.executor_for_block(canonical_db, block).expect("build serial executor");
        executor.enable_block_access_list_builder();
        executor.apply_pre_execution_changes().expect("serial pre-exec");
        for (index, tx) in txs.iter().cloned().enumerate() {
            executor
                .execute_transaction(tx.into())
                .unwrap_or_else(|err| panic!("serial tx {index} failed: {err:?}"));
        }
        let (output, bal) = executor.finish_with_block_access_list().expect("serial post-exec");
        (output, bal.expect("BAL builder was enabled"))
    }

    fn reference_bal_for_empty_block(evm_config: &EthEvmConfig) -> BlockAccessList {
        run_serial_path(evm_config, system_contracts_db(), &empty_amsterdam_block(B256::ZERO), &[])
            .1
    }

    fn reference_bal_for_block(
        evm_config: &EthEvmConfig,
        db: TestDatabase,
        block: &SealedBlock<Block>,
        txs: &[Recovered<TransactionSigned>],
    ) -> BlockAccessList {
        run_serial_path(evm_config, db, block, txs).1
    }

    fn transfer(to: Address, value: u64, nonce: u64, gas_limit: u64) -> Transaction {
        Transaction::Legacy(TxLegacy {
            chain_id: Some(MAINNET.chain.id()),
            nonce,
            gas_price: 1,
            gas_limit,
            to: alloy_primitives::TxKind::Call(to),
            value: U256::from(value),
            input: Default::default(),
        })
    }

    macro_rules! signed_transfer {
        ($key:expr, $signer:expr, $to:expr, $value:expr, $nonce:expr, $gas_limit:expr) => {
            Recovered::new_unchecked(
                sign_tx_with_key_pair($key, transfer($to, $value, $nonce, $gas_limit)),
                $signer,
            )
        };
    }

    fn assert_shadow_equal(
        evm_config: EthEvmConfig,
        canonical_db_template: TestDatabase,
        block_header_only: SealedBlock<Block>,
        txs: Vec<Recovered<TransactionSigned>>,
    ) {
        let (serial, reference_bal) =
            run_serial_path(&evm_config, canonical_db_template.clone(), &block_header_only, &txs);
        let block = empty_amsterdam_block_with_gas_limit(
            compute_block_access_list_hash(&reference_bal),
            block_header_only.header().gas_limit(),
        );
        let bal_out = run_execute_block(
            &Runtime::test(),
            evm_config,
            db_factory(canonical_db_template),
            to_arc_decoded(reference_bal),
            &block,
            txs,
        )
        .unwrap_or_else(|e| panic!("BAL path failed: {e:?}"));

        assert_eq!(serial.receipts, bal_out.receipts, "receipts diverged");
        assert_eq!(serial.gas_used, bal_out.gas_used, "gas used diverged");
        assert_eq!(serial.requests, bal_out.requests, "requests diverged");
        assert_eq!(serial.state, bal_out.state, "execution state diverged");
    }

    #[test]
    fn empty_block_happy_path_round_trip() {
        let evm_config = test_evm_config();
        let input_bal = reference_bal_for_empty_block(&evm_config);
        assert!(!input_bal.is_empty(), "system calls should populate the BAL");
        let block = empty_amsterdam_block(compute_block_access_list_hash(&input_bal));

        let output = run_execute_block(
            &Runtime::test(),
            evm_config,
            db_factory(system_contracts_db()),
            to_arc_decoded(input_bal),
            &block,
            Vec::<Recovered<TransactionSigned>>::new(),
        )
        .expect("empty BAL block should execute");
        assert!(output.receipts.is_empty());
    }

    #[test]
    fn multi_tx_happy_path_round_trip() {
        let evm_config = test_evm_config();
        let carol = Address::repeat_byte(0xca);
        let sender_balance = U256::from(alloy_consensus::constants::ETH_TO_WEI);
        let alice_kp = generate_key(&mut rng());
        let alice = public_key_to_address(alice_kp.public_key());
        let bob_kp = generate_key(&mut rng());
        let bob = public_key_to_address(bob_kp.public_key());
        let txs = vec![
            signed_transfer!(alice_kp, alice, carol, 100, 0, 21_000),
            signed_transfer!(bob_kp, bob, carol, 200, 0, 21_000),
        ];
        let mut db = system_contracts_db();
        insert_funded(&mut db, alice, sender_balance);
        insert_funded(&mut db, bob, sender_balance);
        let reference_bal = reference_bal_for_block(
            &evm_config,
            db.clone(),
            &empty_amsterdam_block(B256::ZERO),
            &txs,
        );
        let block = empty_amsterdam_block(compute_block_access_list_hash(&reference_bal));

        let output = run_execute_block(
            &Runtime::test(),
            evm_config,
            db_factory(db),
            to_arc_decoded(reference_bal),
            &block,
            txs,
        )
        .expect("multi-transaction BAL block should execute");
        assert_eq!(output.receipts.len(), 2);
        assert!(output.gas_used >= 42_000);
    }

    #[test]
    fn shadow_empty_block() {
        assert_shadow_equal(
            test_evm_config(),
            system_contracts_db(),
            empty_amsterdam_block(B256::ZERO),
            Vec::new(),
        );
    }

    #[test]
    fn shadow_multi_value_transfer() {
        let evm_config = test_evm_config();
        let carol = Address::repeat_byte(0xca);
        let sender_balance = U256::from(alloy_consensus::constants::ETH_TO_WEI);
        let alice_kp = generate_key(&mut rng());
        let alice = public_key_to_address(alice_kp.public_key());
        let bob_kp = generate_key(&mut rng());
        let bob = public_key_to_address(bob_kp.public_key());
        let mut db = system_contracts_db();
        insert_funded(&mut db, alice, sender_balance);
        insert_funded(&mut db, bob, sender_balance);
        let txs = vec![
            signed_transfer!(alice_kp, alice, carol, 100, 0, 21_000),
            signed_transfer!(bob_kp, bob, carol, 200, 0, 21_000),
        ];
        assert_shadow_equal(evm_config, db, empty_amsterdam_block(B256::ZERO), txs);
    }

    #[test]
    fn rejects_tx_gas_limit_that_exceeds_remaining_block_gas() {
        let evm_config = test_evm_config();
        let carol = Address::repeat_byte(0xca);
        let sender_balance = U256::from(alloy_consensus::constants::ETH_TO_WEI);
        let alice_kp = generate_key(&mut rng());
        let alice = public_key_to_address(alice_kp.public_key());
        let bob_kp = generate_key(&mut rng());
        let bob = public_key_to_address(bob_kp.public_key());
        let txs = vec![
            signed_transfer!(alice_kp, alice, carol, 100, 0, 990_000),
            signed_transfer!(bob_kp, bob, carol, 200, 0, 990_000),
        ];
        let mut pre_block_db = system_contracts_db();
        insert_funded(&mut pre_block_db, alice, sender_balance);
        insert_funded(&mut pre_block_db, bob, sender_balance);
        let reference_bal = reference_bal_for_block(
            &evm_config,
            pre_block_db.clone(),
            &empty_amsterdam_block(B256::ZERO),
            &txs,
        );
        let block = empty_amsterdam_block_with_gas_limit(
            compute_block_access_list_hash(&reference_bal),
            1_000_000,
        );

        let result = run_execute_block(
            &Runtime::test(),
            evm_config,
            db_factory(pre_block_db),
            to_arc_decoded(reference_bal),
            &block,
            txs,
        );
        match result {
            Err(BalExecutionError::Execution(err)) => assert!(matches!(
                err.as_validation(),
                Some(BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas { .. })
            )),
            other => panic!("expected block gas validation error, got {other:?}"),
        }
    }

    #[test]
    fn shadow_tx_with_revert() {
        let evm_config = test_evm_config();
        let contract = Address::repeat_byte(0xde);
        let key = generate_key(&mut rng());
        let signer = public_key_to_address(key.public_key());
        let mut db = system_contracts_db();
        insert_funded(&mut db, signer, U256::from(alloy_consensus::constants::ETH_TO_WEI));
        db.insert_account_info(
            &contract,
            AccountInfo::default()
                .with_nonce(1)
                .with_code(Bytecode::new_raw(Bytes::from_static(&[0x60, 0x00, 0x60, 0x00, 0xfd]))),
        );
        let tx = signed_transfer!(key, signer, contract, 0, 0, 50_000);
        assert_shadow_equal(evm_config, db, empty_amsterdam_block(B256::ZERO), vec![tx]);
    }

    #[test]
    fn shadow_tx_with_sstore() {
        let evm_config = test_evm_config();
        let contract = Address::repeat_byte(0x55);
        let key = generate_key(&mut rng());
        let signer = public_key_to_address(key.public_key());
        let mut db = system_contracts_db();
        insert_funded(&mut db, signer, U256::from(alloy_consensus::constants::ETH_TO_WEI));
        db.insert_account_info(
            &contract,
            AccountInfo::default().with_nonce(1).with_code(Bytecode::new_raw(Bytes::from_static(
                &[0x60, 0x42, 0x60, 0x00, 0x55, 0x00],
            ))),
        );
        let tx = signed_transfer!(key, signer, contract, 0, 0, 100_000);
        assert_shadow_equal(evm_config, db, empty_amsterdam_block(B256::ZERO), vec![tx]);
    }

    #[test]
    fn returns_built_bal_for_final_hash_mismatch() {
        use alloy_eip7928::AccountChanges;

        let evm_config = test_evm_config();
        let real_bal = reference_bal_for_empty_block(&evm_config);
        let mut tampered_entries: Vec<AccountChanges> = real_bal;
        tampered_entries.push(AccountChanges::new(Address::repeat_byte(0xff)));
        let tampered_bal = AlloyBal::new(tampered_entries);
        let tampered_access_list: BlockAccessList = tampered_bal.clone().into();
        let tampered_hash = compute_block_access_list_hash(&tampered_access_list);
        let block = empty_amsterdam_block(tampered_hash);
        let received = Arc::new(DecodedBal::new(
            tampered_bal.clone(),
            alloy_rlp::encode(&tampered_bal).into(),
        ));

        let (_, built_bal) = run_execute_block_full(
            &Runtime::test(),
            evm_config,
            db_factory(system_contracts_db()),
            received,
            &block,
            Vec::<Recovered<TransactionSigned>>::new(),
        )
        .expect("BAL execution should return the rebuilt list");
        assert_ne!(compute_block_access_list_hash(&built_bal), tampered_hash);
    }

    #[test]
    fn canonical_make_db_failure() {
        let block = empty_amsterdam_block(B256::ZERO);
        let failing_make_db = |_: bool| -> Result<TestDatabase, BalExecutionError> {
            Err(reth_provider::ProviderError::BestBlockNotFound.into())
        };
        let result = run_execute_block(
            &Runtime::test(),
            test_evm_config(),
            failing_make_db,
            to_arc_decoded(BlockAccessList::default()),
            &block,
            Vec::<Recovered<TransactionSigned>>::new(),
        );
        assert!(matches!(result, Err(BalExecutionError::Provider(_))));
    }

    #[test]
    fn worker_tx_recovery_error_becomes_other_error() {
        let evm_config = test_evm_config();
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

        let result = execute_block(
            &Runtime::test(),
            &evm_config,
            &db_factory(system_contracts_db()),
            to_arc_decoded(BlockAccessList::default()),
            evm_env,
            execution_ctx,
            1,
            tx_rx,
            receipt_tx,
        );
        assert!(matches!(result, Err(BalExecutionError::Other(_))));
    }

    #[test]
    fn gas_tracker_non_amsterdam_uses_cumulative_gas() {
        let evm_config = non_amsterdam_evm_config();
        let recipient = Address::repeat_byte(0xca);
        let balance = U256::from(alloy_consensus::constants::ETH_TO_WEI);
        let alice_key = generate_key(&mut rng());
        let alice = public_key_to_address(alice_key.public_key());
        let bob_key = generate_key(&mut rng());
        let bob = public_key_to_address(bob_key.public_key());
        let txs = vec![
            signed_transfer!(alice_key, alice, recipient, 1, 0, 21_000),
            signed_transfer!(bob_key, bob, recipient, 1, 0, 980_000),
        ];
        let mut db = system_contracts_db();
        insert_funded(&mut db, alice, balance);
        insert_funded(&mut db, bob, balance);
        let reference_bal = reference_bal_for_block(
            &evm_config,
            db.clone(),
            &empty_amsterdam_block(B256::ZERO),
            &txs,
        );
        let block = empty_amsterdam_block_with_gas_limit(
            compute_block_access_list_hash(&reference_bal),
            1_000_000,
        );

        let result = run_execute_block(
            &Runtime::test(),
            evm_config,
            db_factory(db),
            to_arc_decoded(reference_bal),
            &block,
            txs,
        );
        assert!(matches!(result, Err(BalExecutionError::Execution(_))));
    }

    #[test]
    fn gas_tracker_caps_oversized_tx_gas_limit_at_tx_gas_limit_cap() {
        const TX_GAS_LIMIT_CAP: u64 = 1 << 24;

        let evm_config = test_evm_config();
        let loop_contract = Address::repeat_byte(0x44);
        let recipient = Address::repeat_byte(0xca);
        let key = generate_key(&mut rng());
        let signer = public_key_to_address(key.public_key());
        let mut db = system_contracts_db();
        insert_funded(&mut db, signer, U256::from(alloy_consensus::constants::ETH_TO_WEI));
        db.insert_account_info(
            &loop_contract,
            AccountInfo::default()
                .with_nonce(1)
                .with_code(Bytecode::new_raw(Bytes::from_static(&[0x5b, 0x60, 0x00, 0x56]))),
        );
        let txs = vec![
            signed_transfer!(key, signer, loop_contract, 0, 0, 13_100_000),
            signed_transfer!(key, signer, recipient, 1, 1, TX_GAS_LIMIT_CAP + 1_000_000),
        ];
        let reference_block = empty_amsterdam_block(B256::ZERO);
        let reference_bal =
            reference_bal_for_block(&evm_config, db.clone(), &reference_block, &txs);
        let block = empty_amsterdam_block(compute_block_access_list_hash(&reference_bal));

        let output = run_execute_block(
            &Runtime::test(),
            evm_config,
            db_factory(db),
            to_arc_decoded(reference_bal),
            &block,
            txs,
        )
        .expect("the capped regular gas limit should fit the remaining block gas");
        assert_eq!(output.receipts.len(), 2);
    }
}

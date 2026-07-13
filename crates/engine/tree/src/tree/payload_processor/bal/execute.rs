//! Parallel BAL block execution.

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

/// Executes one block speculatively against its BAL and commits outputs in transaction order.
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
    let worker_count =
        runtime.bal_streaming_pool().current_num_threads().max(1).min(transaction_count);
    runtime.bal_streaming_pool().in_place_scope(|scope| {
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

struct AbortGuard {
    _tx: Sender<()>,
}

impl AbortGuard {
    fn new() -> (Self, Receiver<()>) {
        let (tx, rx) = crossbeam_channel::bounded(0);
        (Self { _tx: tx }, rx)
    }
}

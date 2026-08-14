use super::BalExecutionError;
use alloy_consensus::Transaction;
use alloy_eip7928::BlockAccessIndex;
use alloy_evm::{
    block::{BlockExecutionError, BlockExecutor, BlockExecutorFactory},
    Evm,
};
use alloy_primitives::Address;
use crossbeam_channel::{Receiver, Sender};
use reth_evm::{execute::ExecutableTxFor, ConfigureEvm, Database, EvmEnvFor, ExecutionCtxFor};
use revm::{database::State, state::bal::Bal as RevmBal};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub(super) enum BalWorkerError {
    /// Worker state or provider setup failed.
    #[error("BAL worker setup failed: {0}")]
    Setup(#[source] BalExecutionError),
    /// Transaction recovery or conversion failed before EVM execution.
    #[error("BAL worker transaction conversion failed: {0}")]
    Transaction(Box<dyn core::error::Error + Send + Sync + 'static>),
}

impl From<BalWorkerError> for BalExecutionError {
    fn from(err: BalWorkerError) -> Self {
        match err {
            BalWorkerError::Setup(err) => err,
            BalWorkerError::Transaction(err) => Self::Other(err),
        }
    }
}

pub(super) struct BalWorkerOutput<R, Tx> {
    pub(super) index: usize,
    pub(super) signer: Address,
    pub(super) tx_gas_limit: u64,
    /// The speculative execution outcome: a transaction result, or the transaction itself so the
    /// ordered commit loop can re-execute it canonically after a speculative failure.
    pub(super) result: Result<R, FailedSpeculation<Tx>>,
}

/// A transaction whose speculative execution against the received BAL failed.
///
/// A speculative failure is a per-transaction outcome, not a block verdict: the commit loop
/// re-executes the transaction on the canonical state and lets block-level admission and
/// post-execution BAL validation decide.
#[derive(Debug)]
pub(super) struct FailedSpeculation<Tx> {
    pub(super) tx: Tx,
    pub(super) source: BlockExecutionError,
}

type WorkerExecutorResult<Cfg> =
    <<Cfg as ConfigureEvm>::BlockExecutorFactory as BlockExecutorFactory>::TxExecutionResult;

type WorkerResultSender<Cfg, Tx> =
    Sender<Result<BalWorkerOutput<WorkerExecutorResult<Cfg>, Tx>, BalWorkerError>>;

#[expect(clippy::too_many_arguments)]
pub(super) fn spawn_worker<'scope, Evm, Tx, Err, DB, MakeDb>(
    scope: &rayon::Scope<'scope>,
    tx_rx: Receiver<(usize, Result<Tx, Err>)>,
    abort_rx: Receiver<()>,
    result_tx: WorkerResultSender<Evm, Tx>,
    evm_config: &'scope Evm,
    make_db: &'scope MakeDb,
    received_bal_revm: Arc<RevmBal>,
    evm_env: EvmEnvFor<Evm>,
    ctx: ExecutionCtxFor<'scope, Evm>,
) where
    Evm: ConfigureEvm + 'scope,
    Tx: ExecutableTxFor<Evm> + Clone + Send + 'scope,
    Err: core::error::Error + Send + Sync + 'static,
    DB: Database + Send + 'scope,
    MakeDb: Fn(bool) -> Result<DB, BalExecutionError> + Sync + 'scope,
{
    scope.spawn(move |_| {
        let worker_result = (|| -> Result<(), BalWorkerError> {
            // Create a database with fill_on_miss=true ensuring misses
            // are inserted for the other workers.
            let database = make_db(true).map_err(BalWorkerError::Setup)?;
            let mut worker_state = State::builder()
                .with_database(database)
                .with_bal(received_bal_revm)
                .with_bundle_update()
                .build();
            let evm = evm_config.evm_with_env(&mut worker_state, evm_env);
            let mut executor = evm_config.create_executor_with_state(evm, ctx.clone());

            loop {
                let (index, tx) = crossbeam_channel::select_biased! {
                    recv(abort_rx) -> _ => break,
                    recv(tx_rx) -> msg => match msg {
                        Ok(ix_tx) => ix_tx,
                        Err(_) => break,
                    },
                };
                let tx = tx.map_err(|e| BalWorkerError::Transaction(Box::new(e)))?;
                let signer = *tx.signer();
                let tx_gas_limit = tx.tx().gas_limit();

                executor.evm_mut().db_mut().set_bal_index(BlockAccessIndex::new(index as u64 + 1));
                // Execute a clone so the transaction survives a speculative failure and can be
                // shipped to the commit loop for canonical re-execution; the worker keeps serving
                // the queue so later transactions stay parallel.
                let result = match executor.execute_transaction_without_commit(tx.clone()) {
                    Ok(result) => Ok(result),
                    Err(source) => Err(FailedSpeculation { tx, source }),
                };

                if result_tx
                    .send(Ok(BalWorkerOutput { index, signer, tx_gas_limit, result }))
                    .is_err()
                {
                    break;
                }
            }

            Ok(())
        })();

        if let Err(err) = worker_result {
            let _ = result_tx.send(Err(err));
        }
    });
}

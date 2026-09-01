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
    /// EVM transaction execution failed.
    #[error("BAL worker EVM execution failed for transaction {tx_index}: {source}")]
    Execution {
        /// Index of the transaction that failed.
        tx_index: usize,
        /// Gas limit of the transaction that failed.
        tx_gas_limit: u64,
        /// The underlying execution error.
        #[source]
        source: BlockExecutionError,
    },
}

impl From<BalWorkerError> for BalExecutionError {
    fn from(err: BalWorkerError) -> Self {
        match err {
            BalWorkerError::Setup(err) => err,
            BalWorkerError::Transaction(err) => Self::Other(err),
            BalWorkerError::Execution { source, .. } => Self::Execution(source),
        }
    }
}

pub(super) struct BalWorkerOutput<R> {
    pub(super) index: usize,
    pub(super) signer: Address,
    pub(super) tx_gas_limit: u64,
    pub(super) result: R,
}

type WorkerExecutorResult<Cfg> =
    <<Cfg as ConfigureEvm>::BlockExecutorFactory as BlockExecutorFactory>::TxExecutionResult;

type WorkerResultSender<Cfg> =
    Sender<Result<BalWorkerOutput<WorkerExecutorResult<Cfg>>, BalWorkerError>>;

#[expect(clippy::too_many_arguments)]
pub(super) fn spawn_worker<'scope, Evm, Tx, Err, DB, MakeDb>(
    scope: &rayon::Scope<'scope>,
    tx_rx: Receiver<(usize, Result<Tx, Err>)>,
    abort_rx: Receiver<()>,
    result_tx: WorkerResultSender<Evm>,
    evm_config: &'scope Evm,
    make_db: &'scope MakeDb,
    received_bal_revm: Arc<RevmBal>,
    evm_env: EvmEnvFor<Evm>,
    ctx: ExecutionCtxFor<'scope, Evm>,
) where
    Evm: ConfigureEvm + 'scope,
    Tx: ExecutableTxFor<Evm> + Send + 'scope,
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
                let (tx_index, tx) = crossbeam_channel::select_biased! {
                    recv(abort_rx) -> _ => break,
                    recv(tx_rx) -> msg => match msg {
                        Ok(ix_tx) => ix_tx,
                        Err(_) => break,
                    },
                };
                let tx = tx.map_err(|e| BalWorkerError::Transaction(Box::new(e)))?;
                let signer = *tx.signer();
                let tx_gas_limit = tx.tx().gas_limit();

                executor
                    .evm_mut()
                    .db_mut()
                    .set_bal_index(BlockAccessIndex::new(tx_index as u64 + 1));
                // An execution failure is a per-transaction result: report it with its
                // index and keep serving the queue so the commit loop can adjudicate it
                // in block order.
                let message = match executor.execute_transaction_without_commit(tx) {
                    Ok(result) => {
                        Ok(BalWorkerOutput { index: tx_index, signer, tx_gas_limit, result })
                    }
                    Err(source) => {
                        Err(BalWorkerError::Execution { tx_index, tx_gas_limit, source })
                    }
                };

                if result_tx.send(message).is_err() {
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

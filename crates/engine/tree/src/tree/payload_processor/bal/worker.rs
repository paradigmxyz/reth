use super::BalExecutionError;
use alloy_eip7928::BlockAccessIndex;
use alloy_primitives::Address;
use crossbeam_channel::{Receiver, Sender};
use reth_errors::ConsensusError;
use reth_evm::{
    BlockExecutionError, BlockExecutor, BlockExecutorFactory, BlockExecutorFor, ConfigureEvm,
    Database, EvmEnvFor, ExecutableTxFor, ExecutionCtxFor,
};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub(super) enum BalWorkerError {
    #[error("BAL worker setup failed: {0}")]
    Setup(#[source] BalExecutionError),
    #[error("BAL worker transaction conversion failed: {0}")]
    Transaction(Box<dyn core::error::Error + Send + Sync + 'static>),
    #[error("BAL worker EVM execution failed: {0}")]
    Execution(BlockExecutionError),
}

impl From<BalWorkerError> for BalExecutionError {
    fn from(err: BalWorkerError) -> Self {
        match err {
            BalWorkerError::Setup(err) => err,
            BalWorkerError::Transaction(err) => Self::Other(err),
            BalWorkerError::Execution(BlockExecutionError::Validation(
                reth_evm::BlockValidationError::BlockAccessListNotCovered,
            )) => Self::Consensus(ConsensusError::BlockAccessListInvalid(
                "block access list does not cover transaction execution".to_string(),
            )),
            BalWorkerError::Execution(err) => Self::Execution(err),
        }
    }
}

pub(super) struct BalWorkerOutput<R> {
    pub(super) index: usize,
    pub(super) signer: Address,
    pub(super) result: R,
}

type WorkerExecutorResult<'a, Cfg> =
    <BlockExecutorFor<'a, Cfg> as BlockExecutor>::TransactionResultWithState;

type WorkerResultSender<'a, Cfg> =
    Sender<Result<BalWorkerOutput<WorkerExecutorResult<'a, Cfg>>, BalWorkerError>>;

#[expect(clippy::too_many_arguments)]
pub(super) fn spawn_worker<'scope, Evm, Tx, Err, DB, MakeDb>(
    scope: &rayon::Scope<'scope>,
    tx_rx: Receiver<(usize, Result<Tx, Err>)>,
    abort_rx: Receiver<()>,
    result_tx: WorkerResultSender<'scope, Evm>,
    evm_config: &'scope Evm,
    make_db: &'scope MakeDb,
    received_bal: Arc<<BlockExecutorFor<'scope, Evm> as BlockExecutor>::BlockAccessList>,
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
            let database = make_db(true).map_err(BalWorkerError::Setup)?;
            let evm = evm_config.evm_with_env(database, evm_env);
            let mut executor = evm_config.block_executor_factory().create_executor(evm, ctx);
            executor.set_block_access_list(received_bal);

            loop {
                let (index, tx) = crossbeam_channel::select_biased! {
                    recv(abort_rx) -> _ => break,
                    recv(tx_rx) -> msg => match msg {
                        Ok(ix_tx) => ix_tx,
                        Err(_) => break,
                    },
                };
                let tx = tx.map_err(|err| BalWorkerError::Transaction(Box::new(err)))?;
                let signer = *tx.signer();
                let (tx_env, _) = tx.into_parts();
                executor.set_block_access_index(BlockAccessIndex::new(index as u64 + 1));
                let result = executor
                    .execute_transaction_without_commit(tx_env)
                    .map_err(BalWorkerError::Execution)?;

                if result_tx.send(Ok(BalWorkerOutput { index, signer, result })).is_err() {
                    break
                }
            }
            Ok(())
        })();

        if let Err(err) = worker_result {
            let _ = result_tx.send(Err(err));
        }
    });
}

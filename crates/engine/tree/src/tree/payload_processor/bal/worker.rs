use super::BalExecutionError;
use alloy_consensus::Transaction;
use alloy_evm::{
    block::{BlockExecutionError, BlockExecutor},
    Evm,
};
use alloy_primitives::Address;
use crossbeam_channel::{Receiver, Sender};
use reth_evm::{
    block::BlockExecutorFactory, execute::ExecutableTxFor, ConfigureEvm, Database, EvmEnvFor,
    ExecutionCtxFor,
};
use revm::database::State;
use revm_state::bal::Bal as RevmBal;
use std::sync::Arc;

pub(super) struct BalWorkerOutput<R> {
    pub(super) index: usize,
    pub(super) signer: Address,
    pub(super) tx_gas_limit: u64,
    pub(super) result: R,
}

type WorkerExecutorResult<Cfg> =
    <<Cfg as ConfigureEvm>::BlockExecutorFactory as BlockExecutorFactory>::TxExecutionResult;

type WorkerResultSender<Cfg> =
    Sender<Result<BalWorkerOutput<WorkerExecutorResult<Cfg>>, BalExecutionError>>;

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
    MakeDb: Fn() -> Result<DB, BalExecutionError> + Sync + 'scope,
{
    scope.spawn(move |_| {
        let worker_result = (|| -> Result<(), BalExecutionError> {
            let mut worker_state = State::builder()
                .with_database(make_db()?)
                .with_bal(received_bal_revm)
                .with_bundle_update()
                .build();
            let evm = evm_config.evm_with_env(&mut worker_state, evm_env);
            let mut executor =
                evm_config.block_executor_factory().create_executor(evm, ctx.clone());

            loop {
                let (index, tx) = crossbeam_channel::select_biased! {
                    recv(abort_rx) -> _ => break,
                    recv(tx_rx) -> msg => match msg {
                        Ok(ix_tx) => ix_tx,
                        Err(_) => break,
                    },
                };
                let tx = tx.map_err(|e| BalExecutionError::Evm(BlockExecutionError::other(e)))?;
                let signer = *tx.signer();
                let tx_gas_limit = tx.tx().gas_limit();

                executor.evm_mut().db_mut().set_bal_index(index as u64 + 1);
                let result = executor
                    .execute_transaction_without_commit(tx)
                    .map_err(BalExecutionError::Evm)?;

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

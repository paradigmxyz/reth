use super::BalExecutionError;
use alloy_consensus::Transaction;
use alloy_eip7928::BlockAccessIndex;
use alloy_primitives::Address;
use crossbeam_channel::{Receiver, Sender};
use reth_evm::{
    BlockExecutionError, BlockExecutor, BlockExecutorFactory, BlockExecutorFor,
    BlockValidationError, ConfigureEvm, Database, EvmEnvFor, ExecutableTxFor, ExecutionCtxFor,
};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub(super) enum BalWorkerError {
    #[error("BAL worker setup failed: {0}")]
    Setup(#[source] BalExecutionError),
    /// Transaction recovery or conversion failed before EVM execution.
    #[error("BAL worker transaction conversion failed for transaction {tx_index}: {source}")]
    Transaction {
        /// Index of the transaction that failed.
        tx_index: usize,
        /// The underlying recovery or conversion error.
        #[source]
        source: Box<dyn core::error::Error + Send + Sync + 'static>,
    },
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
            BalWorkerError::Transaction { source, .. } => {
                Self::Execution(BlockValidationError::Other(source).into())
            }
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
            // Keep the cache-filling database across executor resets so a speculative failure
            // cannot introduce an unindexed provider setup error ahead of its ordered verdict.
            let database = std::rc::Rc::new(std::cell::RefCell::new(
                make_db(true).map_err(BalWorkerError::Setup)?,
            ));
            'worker: loop {
                let evm =
                    evm_config.evm_with_env(WorkerDatabase(database.clone()), evm_env.clone());
                let mut executor =
                    evm_config.block_executor_factory().create_executor(evm, ctx.clone());
                executor.set_block_access_list(Arc::clone(&received_bal));

                loop {
                    let (tx_index, tx) = crossbeam_channel::select_biased! {
                        recv(abort_rx) -> _ => break 'worker,
                        recv(tx_rx) -> msg => match msg {
                            Ok(ix_tx) => ix_tx,
                            Err(_) => break 'worker,
                        },
                    };
                    let tx = match tx {
                        Ok(tx) => tx,
                        Err(source) => {
                            let error =
                                BalWorkerError::Transaction { tx_index, source: Box::new(source) };
                            if result_tx.send(Err(error)).is_err() {
                                break 'worker;
                            }
                            continue;
                        }
                    };
                    let signer = *tx.signer();
                    let tx_gas_limit = tx.tx().gas_limit();

                    executor
                        .set_block_access_index(BlockAccessIndex::from_tx_index(tx_index as u64));
                    let message = match executor.execute_transaction_without_commit(tx) {
                        Ok(result) => {
                            Ok(BalWorkerOutput { index: tx_index, signer, tx_gas_limit, result })
                        }
                        Err(source) => {
                            Err(BalWorkerError::Execution { tx_index, tx_gas_limit, source })
                        }
                    };
                    let failed = message.is_err();
                    if result_tx.send(message).is_err() {
                        break 'worker;
                    }
                    if failed {
                        // The executor trait does not guarantee reuse after an error. Rebuild
                        // its EVM and state before serving more work: the queue can still contain
                        // earlier transactions whose verdict must precede this failure.
                        break;
                    }
                }
            }
            Ok(())
        })();

        if let Err(err) = worker_result {
            let _ = result_tx.send(Err(err));
        }
    });
}

// Executor resets retain the same provider. The wrapper is created and used entirely on one
// worker thread; its shared ownership decouples the database lifetime from each executor.
struct WorkerDatabase<DB>(std::rc::Rc<std::cell::RefCell<DB>>);

impl<DB: Database> Database for WorkerDatabase<DB> {
    type Error = DB::Error;

    fn is_fatal(error: &Self::Error) -> bool {
        DB::is_fatal(error)
    }

    fn get_account(
        &mut self,
        address: &Address,
    ) -> Result<Option<evm2::evm::AccountInfo>, Self::Error> {
        self.0.borrow_mut().get_account(address)
    }
    fn get_code_by_hash(
        &mut self,
        hash: &alloy_primitives::B256,
    ) -> Result<evm2::bytecode::Bytecode, Self::Error> {
        self.0.borrow_mut().get_code_by_hash(hash)
    }
    fn get_storage(
        &mut self,
        address: &Address,
        key: &alloy_primitives::U256,
    ) -> Result<alloy_primitives::U256, Self::Error> {
        self.0.borrow_mut().get_storage(address, key)
    }
    fn get_block_hash(
        &mut self,
        number: &alloy_primitives::U256,
    ) -> Result<alloy_primitives::B256, Self::Error> {
        self.0.borrow_mut().get_block_hash(number)
    }
}

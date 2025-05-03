//! EVM Tracer for executing transactions with a custom inspector and handling results.
use alloy_rpc_types_eth::TransactionInfo;
use reth_evm::{Evm, EvmEnv, EvmFactory, IntoTxEnv};
use revm::{
    context::TxEnv, context_interface::result::ResultAndState, interpreter::InterpreterTypes,
    primitives::hardfork::SpecId, Database, Inspector,
};
use std::{fmt, marker::PhantomData};

/// A struct that represents an EVM tracer for executing transactions with a custom inspector.
pub struct EvmTracer<E, Txs, I, DB, H>
where
    E: EvmFactory,
    DB: InterpreterTypes + Database + Send + Sync + 'static,
    I: Inspector<E::Context<DB>>,
    <DB as revm::Database>::Error: Send + Sync,
    E::Evm<DB, I>: Evm<DB = DB>,
    Txs: Iterator<Item = (TransactionInfo, TxEnv)>,
{
    /// The configured EVM instance.
    evm: E,
    /// Transactions to process.
    transactions: Txs,
    /// Function to create new inspector instances.
    inspector_factory: Box<dyn FnMut() -> I>,
    /// Handler for processing inspector results.
    handler: H,
    /// Environment to use for each transaction.
    env: EvmEnv<E::Spec>,
    /// Database
    db: DB,
    /// Phantom data for generics.
    _marker: PhantomData<I>,
}

impl<E, Txs, I, DB, H> fmt::Debug for EvmTracer<E, Txs, I, DB, H>
where
    E: EvmFactory + fmt::Debug,
    DB: InterpreterTypes + Database + Send + Sync + 'static + fmt::Debug,
    I: Inspector<E::Context<DB>>,
    <DB as revm::Database>::Error: Send + Sync,
    E::Evm<DB, I>: Evm<DB = DB>,
    Txs: Iterator<Item = (TransactionInfo, TxEnv)> + fmt::Debug,
    E::Spec: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EvmTracer")
            .field("evm", &self.evm)
            .field("transactions", &self.transactions)
            .field("env", &self.env)
            .field("db", &self.db)
            .finish()
    }
}

impl<E, Txs, I, DB, H> EvmTracer<E, Txs, I, DB, H>
where
    E: EvmFactory,
    DB: InterpreterTypes + Database + Send + Sync + 'static,
    I: Inspector<E::Context<DB>>,
    <DB as revm::Database>::Error: Send + Sync,
    E::Evm<DB, I>: Evm<DB = DB>,
    Txs: Iterator<Item = (TransactionInfo, TxEnv)>,
    H: FnMut(TransactionInfo, I, ResultAndState<E::HaltReason>) -> Result<(), E::Error<DB::Error>>,
{
    /// Creates a new `EvmTracer`.
    pub fn new(
        evm: E,
        transactions: Txs,
        env: EvmEnv<E::Spec>,
        inspector_factory: impl FnMut() -> I + 'static,
        db: DB,
        handler: H,
    ) -> Self {
        Self {
            evm,
            transactions,
            inspector_factory: Box::new(inspector_factory),
            handler,
            env,
            db,
            _marker: PhantomData,
        }
    }
}

impl<E, Txs, I, DB, H> EvmTracer<E, Txs, I, DB, H>
where
    E: EvmFactory,
    DB: InterpreterTypes + Database + Send + Sync + Clone + 'static,
    I: Inspector<E::Context<DB>> + Clone,
    <DB as revm::Database>::Error: Send + Sync,
    E::Evm<DB, I>: Evm<DB = DB>,
    E::Spec: Into<SpecId> + From<SpecId> + 'static,
    Txs: Iterator<Item = (TransactionInfo, TxEnv)>,
    TxEnv: IntoTxEnv<<E as EvmFactory>::Tx>,
    H: FnMut(TransactionInfo, I, ResultAndState<E::HaltReason>) -> Result<(), E::Error<DB::Error>>,
{
    ///Returns the next value
    pub fn next_tracer(&mut self) -> Option<Result<(), E::Error<DB::Error>>> {
        let (tx_info, tx_env) = self.transactions.next()?;

        // Create a new inspector for this transaction.
        let inspector = (self.inspector_factory)();

        // Execute the transaction.
        let result = {
            let mut evm = self.evm.create_evm_with_inspector(
                self.db.clone(),
                self.env.clone(),
                inspector.clone(),
            );

            match evm.transact(tx_env) {
                Ok(result) => result,
                Err(err) => {
                    // Handle the error or log it as needed
                    eprintln!("Error occurred: {err:?}");
                    return None;
                }
            }
        };

        Some((self.handler)(tx_info, inspector, result))
    }
}

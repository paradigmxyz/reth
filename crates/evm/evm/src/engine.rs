use std::collections::BTreeMap;
use crate::{execute::ExecutableTxFor, ConfigureEvm, EvmEnvFor, ExecutionCtxFor, TxEnvFor};
use crate::execute::WithTxEnv;

/// [`ConfigureEvm`] extension providing methods for executing payloads.
pub trait ConfigureEngineEvm<ExecutionData>: ConfigureEvm {
    /// Returns an [`crate::EvmEnv`] for the given payload.
    fn evm_env_for_payload(&self, payload: &ExecutionData) -> Result<EvmEnvFor<Self>, Self::Error>;

    /// Returns an [`ExecutionCtxFor`] for the given payload.
    fn context_for_payload<'a>(
        &self,
        payload: &'a ExecutionData,
    ) -> Result<ExecutionCtxFor<'a, Self>, Self::Error>;

    /// Returns an [`ExecutableTxIterator`] for the given payload.
    fn tx_iterator_for_payload(
        &self,
        payload: &ExecutionData,
    ) -> Result<impl ExecutableTxIterator<Self>, Self::Error>;
}

/// Iterator over executable transactions.
pub trait ExecutableTxIterator<Evm: ConfigureEvm>:
    Iterator<Item = Result<Self::Tx, Self::Error>> + Send + 'static
{
    /// The executable transaction type iterator yields.
    type Tx: ExecutableTxFor<Evm> + Clone + Send + Sync + 'static;
    /// Errors that may occur while recovering or decoding transactions.
    type Error: core::error::Error + Send + Sync + 'static;
}

impl<Evm: ConfigureEvm, Tx, Err, T> ExecutableTxIterator<Evm> for T
where
    Tx: ExecutableTxFor<Evm> + Clone + Send + Sync + 'static,
    Err: core::error::Error + Send + Sync + 'static,
    T: Iterator<Item = Result<Tx, Err>> + Send + 'static,
{
    type Tx = Tx;
    type Error = Err;
}

pub trait ExecutableTxIterator3<Evm: ConfigureEvm>
{
    /// The executable transaction type iterator yields.
    type Tx: ExecutableTxFor<Evm> + Clone + Send + Sync + 'static;
    /// Errors that may occur while recovering or decoding transactions.
    type Error: core::error::Error + Send + Sync + 'static;

    /// Yields the prepared [`PreparedTx`]
    ///
    /// This can either be the next ready transaction, or any transaction out of order.
    ///
    /// ## Guarantees
    ///
    /// Implementers must guarantee that [`PreparedTx::Next`] is yielded in the correct order as they exist in the initial set (Block.)
    ///
    /// A yielded transaction `tx` can appear at most twice:
    ///  - First as [`PreparedTx::Prepared`] if we haven't yielded the previous tx (`tx.index - 1`) as [`PreparedTx::Next`]
    /// - Always as [`PreparedTx::Next`] if it's the next transaction in line.
    fn next<'a>(&'a mut self) -> Option<Result<PreparedTx<'a, WithTxEnv<TxEnvFor<Evm>, Self::Tx>>, Self::Error>>;

    /// The total number of transactions in this iterator
    fn total_tx_count(&self) -> usize;
}

#[derive(Debug)]
enum PreparedTx<'a, Tx> {
    Next(Tx),
    Prepared(&'a Tx)
}

/// Drives an underlying underlying iterator and collects the transactions.
///
/// Transactions can be yielded out of order by the iterator.
pub struct CollectExecutableTxIterator<Evm, Iter, Tx, Err>
where Evm: ConfigureEvm,
    Iter: Iterator<Item = (usize, Result<WithTxEnv<TxEnvFor<Evm>, Tx>, Err>)>
{
    /// The iterator that yields prepared tx in any order
    iter: Iter,
    /// Prepared transaction that wait for an ancestor to become available from the `iter`.
    buffered: BTreeMap<usize, WithTxEnv<TxEnvFor<Evm>, Tx>>,
    current_idx: usize,
}

impl<Evm, Iter, Tx, Err> ExecutableTxIterator3<Evm> for CollectExecutableTxIterator<Evm, Iter, Tx, Err>
where
    Evm: ConfigureEvm,
    Iter: Iterator<Item = (usize, Result<WithTxEnv<TxEnvFor<Evm>, Tx>, Err>)>,
    Tx: ExecutableTxFor<Evm> + Clone + Send + Sync + 'static,
    Err: core::error::Error + Send + Sync + 'static,
{
    type Tx = Tx;
    type Error = Err;

    fn next(&mut self) -> Option<Result<PreparedTx<WithTxEnv<TxEnvFor<Evm>, Self::Tx>>, Self::Error>> {
        // First check if the next transaction in sequence is already buffered
        if let Some(tx) = self.buffered.remove(&self.current_idx) {
            self.current_idx += 1;
            return Some(Ok(PreparedTx::Next(tx)));
        }

        // Pull next item from iterator
        let (idx, result) = self.iter.next()?;

        match result {
            Err(err) => Some(Err(err)),
            Ok(tx) => {
                if idx == self.current_idx {
                    // This is the next transaction in sequence
                    self.current_idx += 1;
                    Some(Ok(PreparedTx::Next(tx)))
                } else {
                    // This transaction is out of order, buffer it and return as Prepared
                    self.buffered.insert(idx, tx);
                    // Return a reference to the buffered transaction
                    // SAFETY: we just inserted at this index
                    let tx_ref = self.buffered.get(&idx).unwrap();
                    Some(Ok(PreparedTx::Prepared(tx_ref)))
                }
            }
        }
    }

    fn total_tx_count(&self) -> usize {
        todo!()
    }
}

// #[cfg(feature = "rayon")]
mod parallel {
    use rayon::prelude::IntoParallelIterator;
    use crate::{ConfigureEvm, TxEnvFor};
    use crate::execute::{ExecutableTxFor, WithTxEnv};

    pub struct RayonTxIter<Evm, Iter, Tx, Err>
    where Evm: ConfigureEvm,
          Tx: ExecutableTxFor<Evm> + Clone + Send + Sync + 'static,
    {
        from_tx: std::sync::mpsc::Receiver< (usize, Result<WithTxEnv<TxEnvFor<Evm>, Tx>, Err>)>
    }

    impl<Evm,Iter, Tx, Err> RayonTxIter<Evm, Iter, Tx, Err> {

        fn spawn<I>(iter: I) -> Self
            where I: IntoParallelIterator<Item = (usize, Result<WithTxEnv<TxEnvFor<Evm>, Tx>, Err>)>
        {
            let (from_tx, rx) = std::sync::mpsc::channel();

            rayon::spawn(move || {


            })


            todo!()
        }

    }

}
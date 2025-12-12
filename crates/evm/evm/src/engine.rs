use crate::{execute::ExecutableTxFor, ConfigureEvm, EvmEnvFor, ExecutionCtxFor};

#[cfg(feature = "std")]
use rayon::prelude::*;

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

/// A helper trait representing a pair of a "raw" transactions iterator and a closure that can be
/// used to convert them to an executable transaction. This tuple is used in the engine to
/// parallelize heavy work like decoding or recovery.
pub trait ExecutableTxTuple: Into<(Self::IntoIter, Self::Convert)> + Send + 'static {
    /// Raw transaction that can be converted to an [`ExecutableTxTuple::Tx`]
    ///
    /// This can be any type that can be converted to an [`ExecutableTxTuple::Tx`]. For example,
    /// an unrecovered transaction or just the transaction bytes.
    type RawTx: Send + Sync + 'static;
    /// The executable transaction type iterator yields.
    type Tx: Clone + Send + Sync + 'static;
    /// Errors that may occur while recovering or decoding transactions.
    type Error: core::error::Error + Send + Sync + 'static;

    /// Iterator over [`ExecutableTxTuple::Tx`].
    #[cfg(feature = "std")]
    type IntoIter: IntoParallelIterator<Item = Self::RawTx, Iter: IndexedParallelIterator>
        + Send
        + 'static;
    /// Iterator over [`ExecutableTxTuple::Tx`].
    #[cfg(not(feature = "std"))]
    type IntoIter: IntoIterator<Item = Self::RawTx> + Send + 'static;
    /// Closure that can be used to convert a [`ExecutableTxTuple::RawTx`] to a
    /// [`ExecutableTxTuple::Tx`]. This might involve heavy work like decoding or recovery
    /// and will be parallelized in the engine.
    type Convert: Fn(Self::RawTx) -> Result<Self::Tx, Self::Error> + Send + Sync + 'static;
}

macro_rules! maybe_parallel_iter {
    ($($i_bound:tt)*) => {
        impl<RawTx, Tx, Err, I, F> ExecutableTxTuple for (I, F)
        where
            RawTx: Send + Sync + 'static,
            Tx: Clone + Send + Sync + 'static,
            Err: core::error::Error + Send + Sync + 'static,
            I: $($i_bound)* + Send + 'static,
            F: Fn(RawTx) -> Result<Tx, Err> + Send + Sync + 'static,
        {
            type RawTx = RawTx;
            type Tx = Tx;
            type Error = Err;

            type IntoIter = I;
            type Convert = F;
        }
    };
}

#[cfg(feature = "std")]
maybe_parallel_iter!(IntoParallelIterator<Item = RawTx, Iter: IndexedParallelIterator>);
#[cfg(not(feature = "std"))]
maybe_parallel_iter!(IntoIterator<Item = RawTx>);

/// Iterator over executable transactions.
pub trait ExecutableTxIterator<Evm: ConfigureEvm>:
    ExecutableTxTuple<Tx: ExecutableTxFor<Evm>>
{
}

impl<T, Evm: ConfigureEvm> ExecutableTxIterator<Evm> for T where
    T: ExecutableTxTuple<Tx: ExecutableTxFor<Evm>>
{
}

use crate::{execute::ExecutableTxFor, ConfigureEvm, EvmEnvFor, ExecutionCtxFor};

/// [`ConfigureEvm`] extension providing methods for executing payloads.
pub trait ConfigureEngineEvm<ExecutionData>: ConfigureEvm {
    /// Returns an [`EvmEnvFor`] for the given payload.
    fn evm_env_for_payload(&self, payload: &ExecutionData) -> EvmEnvFor<Self>;

    /// Returns an [`ExecutionCtxFor`] for the given payload.
    fn context_for_payload<'a>(&self, payload: &'a ExecutionData) -> ExecutionCtxFor<'a, Self>;

    /// Returns an [`ExecutableTxIterator`] for the given payload.
    fn tx_iterator_for_payload(&self, payload: &ExecutionData) -> impl ExecutableTxIterator<Self>;
}

/// Iterator over executable transactions.
pub trait ExecutableTxIterator<Evm: ConfigureEvm>:
    Iterator<Item = Result<Self::Tx, Self::Error>> + Send + 'static
{
    /// The executable transaction type iterator yields.
    type Tx: ExecutableTxFor<Evm> + Clone + Send + 'static;
    /// Errors that may occur while recovering or decoding transactions.
    type Error: core::error::Error + Send + Sync + 'static;
}

impl<Evm: ConfigureEvm, Tx, Err, T> ExecutableTxIterator<Evm> for T
where
    Tx: ExecutableTxFor<Evm> + Clone + Send + 'static,
    Err: core::error::Error + Send + Sync + 'static,
    T: Iterator<Item = Result<Tx, Err>> + Send + 'static,
{
    type Tx = Tx;
    type Error = Err;
}

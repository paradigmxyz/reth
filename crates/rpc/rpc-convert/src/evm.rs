use reth_evm::EvmEnv;
use std::{convert::Infallible, fmt::Debug};

/// Performs an [`EvmEnv`] to [`EvmEnv`] conversion accompanied by the `TxReq`.
///
/// The `TxReq` represents the transaction request this [`EvmEnv`] has been created for.
///
/// Allows for custom effects on the [`EvmEnv`]. Default implementation `()` passes the input
/// argument with no change.
pub trait EvmEnvConverter<TxReq>: Clone + Debug + Unpin + Send + Sync + 'static {
    /// An associated error that can occur during the conversion.
    type Err;

    /// Performs the conversion.
    ///
    /// See [`EvmEnvConverter`] for more details.
    fn convert_evm_env<Spec>(
        &self,
        request: &TxReq,
        evm_env: EvmEnv<Spec>,
    ) -> Result<EvmEnv<Spec>, Self::Err>;
}

impl<TxReq> EvmEnvConverter<TxReq> for () {
    type Err = Infallible;

    fn convert_evm_env<Spec>(
        &self,
        _request: &TxReq,
        evm_env: EvmEnv<Spec>,
    ) -> Result<EvmEnv<Spec>, Self::Err> {
        Ok(evm_env)
    }
}

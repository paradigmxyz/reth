//! Helper traits to wrap generic l1 errors, in network specific error type configured in
//! `reth_rpc_eth_api::EthApiTypes`.

use crate::EthApiError;
use reth_errors::ProviderError;
use reth_evm::{ConfigureEvm, EvmErrorFor, HaltReasonFor};
use revm::context_interface::result::HaltReason;

use super::RpcInvalidTransactionError;

/// Helper trait to wrap core [`EthApiError`].
pub trait FromEthApiError: From<EthApiError> {
    /// Converts from error via [`EthApiError`].
    fn from_eth_err<E>(err: E) -> Self
    where
        EthApiError: From<E>;
}

impl<T> FromEthApiError for T
where
    T: From<EthApiError>,
{
    fn from_eth_err<E>(err: E) -> Self
    where
        EthApiError: From<E>,
    {
        T::from(EthApiError::from(err))
    }
}

/// Helper trait to wrap core [`EthApiError`].
pub trait IntoEthApiError: Into<EthApiError> {
    /// Converts into error via [`EthApiError`].
    fn into_eth_err<E>(self) -> E
    where
        E: FromEthApiError;
}

impl<T> IntoEthApiError for T
where
    EthApiError: From<T>,
{
    fn into_eth_err<E>(self) -> E
    where
        E: FromEthApiError,
    {
        E::from_eth_err(self)
    }
}

/// Helper trait to access wrapped core error.
pub trait AsEthApiError {
    /// Returns reference to [`EthApiError`], if this an error variant inherited from core
    /// functionality.
    fn as_err(&self) -> Option<&EthApiError>;

    /// Returns `true` if error is
    /// [`RpcInvalidTransactionError::GasTooHigh`].
    fn is_gas_too_high(&self) -> bool {
        if let Some(err) = self.as_err() {
            return err.is_gas_too_high()
        }

        false
    }

    /// Returns `true` if error is
    /// [`RpcInvalidTransactionError::GasTooLow`].
    fn is_gas_too_low(&self) -> bool {
        if let Some(err) = self.as_err() {
            return err.is_gas_too_low()
        }

        false
    }
}

impl AsEthApiError for EthApiError {
    fn as_err(&self) -> Option<&EthApiError> {
        Some(self)
    }
}

/// Helper trait to convert from revm errors.
pub trait FromEvmError<Evm: ConfigureEvm>:
    From<EvmErrorFor<Evm, ProviderError>> + FromEvmHalt<HaltReasonFor<Evm>>
{
    /// Converts from EVM error to this type.
    fn from_evm_err(err: EvmErrorFor<Evm, ProviderError>) -> Self {
        err.into()
    }
}

impl<T, Evm> FromEvmError<Evm> for T
where
    T: From<EvmErrorFor<Evm, ProviderError>> + FromEvmHalt<HaltReasonFor<Evm>>,
    Evm: ConfigureEvm,
{
}

/// Helper trait to convert from revm errors.
pub trait FromEvmHalt<Halt> {
    /// Converts from EVM halt to this type.
    fn from_evm_halt(halt: Halt, gas_limit: u64) -> Self;
}

impl FromEvmHalt<HaltReason> for EthApiError {
    fn from_evm_halt(halt: HaltReason, gas_limit: u64) -> Self {
        RpcInvalidTransactionError::halt(halt, gas_limit).into()
    }
}

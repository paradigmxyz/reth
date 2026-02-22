//! Helper traits to wrap generic l1 errors, in network specific error type configured in
//! `reth_rpc_eth_api::EthApiTypes`.

use crate::{simulate::EthSimulateError, EthApiError, RevertError};
use alloy_primitives::Bytes;
use reth_errors::ProviderError;
use reth_evm::{ConfigureEvm, EvmErrorFor, HaltReasonFor};
use reth_revm::db::bal::EvmDatabaseError;
use revm::{context::result::ExecutionResult, context_interface::result::HaltReason};

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

    /// Returns [`EthSimulateError`] if this error maps to a simulate-specific error code.
    fn as_simulate_error(&self) -> Option<EthSimulateError> {
        let err = self.as_err()?;
        match err {
            EthApiError::InvalidTransaction(tx_err) => match tx_err {
                RpcInvalidTransactionError::NonceTooLow { tx, state } => {
                    Some(EthSimulateError::NonceTooLow { tx: *tx, state: *state })
                }
                RpcInvalidTransactionError::NonceTooHigh => Some(EthSimulateError::NonceTooHigh),
                RpcInvalidTransactionError::FeeCapTooLow => {
                    Some(EthSimulateError::BaseFeePerGasTooLow)
                }
                RpcInvalidTransactionError::GasTooLow => Some(EthSimulateError::IntrinsicGasTooLow),
                RpcInvalidTransactionError::InsufficientFunds { cost, balance } => {
                    Some(EthSimulateError::InsufficientFunds { cost: *cost, balance: *balance })
                }
                RpcInvalidTransactionError::SenderNoEOA => Some(EthSimulateError::SenderNotEOA),
                RpcInvalidTransactionError::MaxInitCodeSizeExceeded => {
                    Some(EthSimulateError::MaxInitCodeSizeExceeded)
                }
                _ => None,
            },
            _ => None,
        }
    }
}

impl AsEthApiError for EthApiError {
    fn as_err(&self) -> Option<&EthApiError> {
        Some(self)
    }
}

/// Helper trait to convert from revm errors.
pub trait FromEvmError<Evm: ConfigureEvm>:
    From<EvmErrorFor<Evm, EvmDatabaseError<ProviderError>>>
    + FromEvmHalt<HaltReasonFor<Evm>>
    + FromRevert
{
    /// Converts from EVM error to this type.
    fn from_evm_err(err: EvmErrorFor<Evm, EvmDatabaseError<ProviderError>>) -> Self {
        err.into()
    }

    /// Converts from EVM error to this type with transaction index context.
    ///
    /// This preserves the original error's RPC code and data while adding
    /// transaction index information to the error message.
    fn from_evm_err_at_index(
        err: EvmErrorFor<Evm, EvmDatabaseError<ProviderError>>,
        tx_index: usize,
    ) -> Self;

    /// Ensures the execution result is successful or returns an error,
    fn ensure_success(result: ExecutionResult<HaltReasonFor<Evm>>) -> Result<Bytes, Self> {
        match result {
            ExecutionResult::Success { output, .. } => Ok(output.into_data()),
            ExecutionResult::Revert { output, .. } => Err(Self::from_revert(output)),
            ExecutionResult::Halt { reason, gas_used } => {
                Err(Self::from_evm_halt(reason, gas_used))
            }
        }
    }
}

impl<T, Evm> FromEvmError<Evm> for T
where
    T: From<EvmErrorFor<Evm, EvmDatabaseError<ProviderError>>>
        + FromEvmHalt<HaltReasonFor<Evm>>
        + FromRevert,
    Evm: ConfigureEvm,
    EthApiError: From<EvmErrorFor<Evm, EvmDatabaseError<ProviderError>>>,
    Self: From<EthApiError>,
{
    fn from_evm_err_at_index(
        err: EvmErrorFor<Evm, EvmDatabaseError<ProviderError>>,
        tx_index: usize,
    ) -> Self {
        EthApiError::from(err).with_tx_index(tx_index).into()
    }
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

/// Helper trait to construct errors from unexpected reverts.
pub trait FromRevert {
    /// Constructs an error from revert bytes.
    ///
    /// This is only invoked when revert was unexpected (`eth_call`, `eth_estimateGas`, etc).
    fn from_revert(output: Bytes) -> Self;
}

impl FromRevert for EthApiError {
    fn from_revert(output: Bytes) -> Self {
        RpcInvalidTransactionError::Revert(RevertError::new(output)).into()
    }
}

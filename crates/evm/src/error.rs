use revm_primitives::{EVMError, InvalidTransaction};

/// Abstraction over transaction validation error.
pub trait InvalidTxError: core::error::Error + Send + Sync + 'static {
    /// Returns whether the error cause by transaction having a nonce lower than expected.
    fn is_nonce_too_low(&self) -> bool;
}

impl InvalidTxError for InvalidTransaction {
    fn is_nonce_too_low(&self) -> bool {
        matches!(self, Self::NonceTooLow { .. })
    }
}

/// Abstraction over errors that can occur during EVM execution.
///
/// It's assumed that errors can occur either because of an invalid transaction, meaning that other
/// transaction might still result in successful execution, or because of a general EVM
/// misconfiguration.
///
/// If caller occurs a error different from [`EvmError::InvalidTransaction`], it should most likely
/// be treated as fatal error flagging some EVM misconfiguration.
pub trait EvmError: core::error::Error + Send + Sync + 'static {
    /// Errors which might occur as a result of an invalid transaction. i.e unrelated to general EVM
    /// configuration.
    type InvalidTransaction: InvalidTxError;

    /// Returns the [`EvmError::InvalidTransaction`] if the error is an invalid transaction error.
    fn as_invalid_tx_err(&self) -> Option<&Self::InvalidTransaction>;

    /// Returns `true` if the error is an invalid transaction error.
    fn is_invalid_tx_err(&self) -> bool {
        self.as_invalid_tx_err().is_some()
    }
}

impl<DBError> EvmError for EVMError<DBError>
where
    DBError: core::error::Error + Send + Sync + 'static,
{
    type InvalidTransaction = InvalidTransaction;

    fn as_invalid_tx_err(&self) -> Option<&Self::InvalidTransaction> {
        match self {
            Self::Transaction(err) => Some(err),
            _ => None,
        }
    }
}

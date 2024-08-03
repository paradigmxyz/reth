//! Commonly used code snippets

use reth_primitives::{Bytes, PooledTransactionsElement, PooledTransactionsElementEcRecovered};

use super::{EthApiError, EthResult};

/// Recovers a [`PooledTransactionsElementEcRecovered`] from an enveloped encoded byte stream.
///
/// See [`PooledTransactionsElement::decode_enveloped`]
pub fn recover_raw_transaction(data: Bytes) -> EthResult<PooledTransactionsElementEcRecovered> {
    if data.is_empty() {
        return Err(EthApiError::EmptyRawTransactionData)
    }

    let transaction = PooledTransactionsElement::decode_enveloped(&mut data.as_ref())
        .map_err(|_| EthApiError::FailedToDecodeSignedTransaction)?;

    transaction.try_into_ecrecovered().or(Err(EthApiError::InvalidTransactionSignature))
}

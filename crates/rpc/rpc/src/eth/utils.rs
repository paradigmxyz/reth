//! Commonly used code snippets

use crate::eth::error::{EthApiError, EthResult};
use reth_primitives::{Bytes, TransactionSigned, TransactionSignedEcRecovered};

/// Recovers a [TransactionSignedEcRecovered] from an enveloped encoded byte stream.
///
/// See [TransactionSigned::decode_enveloped]
pub(crate) fn recover_raw_transaction(data: Bytes) -> EthResult<TransactionSignedEcRecovered> {
    if data.is_empty() {
        return Err(EthApiError::EmptyRawTransactionData)
    }

    let transaction = TransactionSigned::decode_enveloped(data)
        .map_err(|_| EthApiError::FailedToDecodeSignedTransaction)?;

    transaction.into_ecrecovered().ok_or(EthApiError::InvalidTransactionSignature)
}

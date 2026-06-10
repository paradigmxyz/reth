use core::{fmt::Debug, future::Future};

use alloy_consensus::{EthereumTxEnvelope, SignableTransaction, TxEip4844};
use alloy_network::TxSigner;
use alloy_primitives::Signature;
use alloy_rpc_types_eth::TransactionRequest;

/// Error for [`SignableTxRequest`] trait.
#[derive(Debug, thiserror::Error)]
pub enum SignTxRequestError {
    /// The transaction request is invalid.
    #[error("invalid transaction request")]
    InvalidTransactionRequest,

    /// The signer is not supported.
    #[error(transparent)]
    SignerNotSupported(#[from] alloy_signer::Error),
}

/// An abstraction over transaction requests that can be signed.
pub trait SignableTxRequest<T>: Send + Sync + 'static {
    /// Attempts to build a transaction request and sign it with the given signer.
    fn try_build_and_sign(
        self,
        signer: impl TxSigner<Signature> + Send,
    ) -> impl Future<Output = Result<T, SignTxRequestError>> + Send;
}

impl SignableTxRequest<EthereumTxEnvelope<TxEip4844>> for TransactionRequest {
    async fn try_build_and_sign(
        self,
        signer: impl TxSigner<Signature> + Send,
    ) -> Result<EthereumTxEnvelope<TxEip4844>, SignTxRequestError> {
        let mut tx =
            self.build_typed_tx().map_err(|_| SignTxRequestError::InvalidTransactionRequest)?;
        let signature = signer.sign_transaction(&mut tx).await?;
        Ok(tx.into_signed(signature).into())
    }
}

use std::{fmt::Debug, future::Future};

use alloy_consensus::{
    EthereumTxEnvelope, EthereumTypedTransaction, SignableTransaction, TxEip4844,
};
use alloy_json_rpc::RpcObject;
use alloy_network::{
    primitives::HeaderResponse, Network, ReceiptResponse, TransactionResponse, TxSigner,
};
use alloy_primitives::Signature;
use alloy_rpc_types_eth::TransactionRequest;

/// RPC types used by the `eth_` RPC API.
///
/// This is a subset of [`Network`] trait with only RPC response types kept.
pub trait RpcTypes: Send + Sync + Clone + Unpin + Debug + 'static {
    /// Header response type.
    type Header: RpcObject + HeaderResponse;
    /// Receipt response type.
    type Receipt: RpcObject + ReceiptResponse;
    /// Transaction response type.
    type TransactionResponse: RpcObject + TransactionResponse;
    /// Transaction response type.
    type TransactionRequest: RpcObject + AsRef<TransactionRequest> + AsMut<TransactionRequest>;
}

impl<T> RpcTypes for T
where
    T: Network<TransactionRequest: AsRef<TransactionRequest> + AsMut<TransactionRequest>> + Unpin,
{
    type Header = T::HeaderResponse;
    type Receipt = T::ReceiptResponse;
    type TransactionResponse = T::TransactionResponse;
    type TransactionRequest = T::TransactionRequest;
}

/// Adapter for network specific transaction response.
pub type RpcTransaction<T> = <T as RpcTypes>::TransactionResponse;

/// Adapter for network specific receipt response.
pub type RpcReceipt<T> = <T as RpcTypes>::Receipt;

/// Adapter for network specific header response.
pub type RpcHeader<T> = <T as RpcTypes>::Header;

/// Adapter for network specific block type.
pub type RpcBlock<T> = alloy_rpc_types_eth::Block<RpcTransaction<T>, RpcHeader<T>>;

/// Adapter for network specific transaction request.
pub type RpcTxReq<T> = <T as RpcTypes>::TransactionRequest;

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
        let signed = match tx {
            EthereumTypedTransaction::Legacy(tx) => {
                EthereumTxEnvelope::Legacy(tx.into_signed(signature))
            }
            EthereumTypedTransaction::Eip2930(tx) => {
                EthereumTxEnvelope::Eip2930(tx.into_signed(signature))
            }
            EthereumTypedTransaction::Eip1559(tx) => {
                EthereumTxEnvelope::Eip1559(tx.into_signed(signature))
            }
            EthereumTypedTransaction::Eip4844(tx) => {
                EthereumTxEnvelope::Eip4844(TxEip4844::from(tx).into_signed(signature))
            }
            EthereumTypedTransaction::Eip7702(tx) => {
                EthereumTxEnvelope::Eip7702(tx.into_signed(signature))
            }
        };
        Ok(signed)
    }
}

#[cfg(feature = "op")]
impl SignableTxRequest<op_alloy_consensus::OpTxEnvelope>
    for op_alloy_rpc_types::OpTransactionRequest
{
    async fn try_build_and_sign(
        self,
        signer: impl TxSigner<Signature> + Send,
    ) -> Result<op_alloy_consensus::OpTxEnvelope, SignTxRequestError> {
        let mut tx =
            self.build_typed_tx().map_err(|_| SignTxRequestError::InvalidTransactionRequest)?;
        let signature = signer.sign_transaction(&mut tx).await?;
        let signed = match tx {
            op_alloy_consensus::OpTypedTransaction::Legacy(tx) => {
                op_alloy_consensus::OpTxEnvelope::Legacy(tx.into_signed(signature))
            }
            op_alloy_consensus::OpTypedTransaction::Eip2930(tx) => {
                op_alloy_consensus::OpTxEnvelope::Eip2930(tx.into_signed(signature))
            }
            op_alloy_consensus::OpTypedTransaction::Eip1559(tx) => {
                op_alloy_consensus::OpTxEnvelope::Eip1559(tx.into_signed(signature))
            }
            op_alloy_consensus::OpTypedTransaction::Eip7702(tx) => {
                op_alloy_consensus::OpTxEnvelope::Eip7702(tx.into_signed(signature))
            }
            op_alloy_consensus::OpTypedTransaction::Deposit(_) => {
                return Err(SignTxRequestError::InvalidTransactionRequest);
            }
        };
        Ok(signed)
    }
}

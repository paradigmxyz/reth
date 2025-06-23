use alloy_json_rpc::RpcObject;
use alloy_network::{Network, ReceiptResponse, TransactionResponse};

/// RPC types used by the `eth_` RPC API.
///
/// This is a subset of [`Network`] trait with only RPC response types kept.
pub trait RpcTypes {
    /// Header response type.
    type Header: RpcObject;
    /// Receipt response type.
    type Receipt: RpcObject + ReceiptResponse;
    /// Transaction response type.
    type TransactionResponse: RpcObject + TransactionResponse;
    /// Transaction response type.
    type TransactionRequest: RpcObject;
}

impl<T> RpcTypes for T
where
    T: Network,
{
    type Header = T::HeaderResponse;
    type Receipt = T::ReceiptResponse;
    type TransactionResponse = T::TransactionResponse;
    type TransactionRequest = T::TransactionRequest;
}

/// Adapter for network specific transaction response.
pub type RpcTransaction<T> = <T as RpcTypes>::TransactionResponse;

/// Adapter for network specific transaction request.
pub type RpcTxReq<T> = <T as RpcTypes>::TransactionRequest;

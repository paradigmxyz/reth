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
    type Transaction: RpcObject + TransactionResponse;
}

impl<T> RpcTypes for T
where
    T: Network,
{
    type Header = T::HeaderResponse;
    type Receipt = T::ReceiptResponse;
    type Transaction = T::TransactionResponse;
}

/// Adapter for network specific transaction type.
pub type RpcTransaction<T> = <T as RpcTypes>::Transaction;

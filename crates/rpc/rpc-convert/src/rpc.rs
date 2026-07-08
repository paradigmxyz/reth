use std::fmt::Debug;

use alloy_json_rpc::RpcObject;
use alloy_network::{primitives::HeaderResponse, Network, ReceiptResponse, TransactionResponse};
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

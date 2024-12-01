//! Trait for specifying `eth` network dependent API types.

use std::{
    error::Error,
    fmt::{self},
};

use alloy_network::Network;
use alloy_rpc_types_eth::Block;
use reth_primitives::TransactionSigned;
use reth_provider::{ReceiptProvider, TransactionsProvider};
use reth_rpc_types_compat::TransactionCompat;

use crate::{AsEthApiError, FromEthApiError, FromEvmError, RpcNodeCore};

/// Network specific `eth` API types.
pub trait EthApiTypes: Send + Sync + Clone {
    /// Extension of [`FromEthApiError`], with network specific errors.
    type Error: Into<jsonrpsee_types::error::ErrorObject<'static>>
        + FromEthApiError
        + AsEthApiError
        + FromEvmError
        + Error
        + Send
        + Sync;
    /// Blockchain primitive types, specific to network, e.g. block and transaction.
    type NetworkTypes: Network<HeaderResponse = alloy_rpc_types_eth::Header>;
    /// Conversion methods for transaction RPC type.
    type TransactionCompat: Send + Sync + Clone + fmt::Debug;

    /// Returns reference to transaction response builder.
    fn tx_resp_builder(&self) -> &Self::TransactionCompat;
}

/// Adapter for network specific transaction type.
pub type RpcTransaction<T> = <T as Network>::TransactionResponse;

/// Adapter for network specific block type.
pub type RpcBlock<T> = Block<RpcTransaction<T>, <T as Network>::HeaderResponse>;

/// Adapter for network specific receipt type.
pub type RpcReceipt<T> = <T as Network>::ReceiptResponse;

/// Adapter for network specific error type.
pub type RpcError<T> = <T as EthApiTypes>::Error;

/// Helper trait holds necessary trait bounds on [`EthApiTypes`] to implement `eth` API.
pub trait FullEthApiTypes
where
    Self: RpcNodeCore<
            Provider: TransactionsProvider<Transaction = TransactionSigned>
                          + ReceiptProvider<Receipt = reth_primitives::Receipt>,
        > + EthApiTypes<
            TransactionCompat: TransactionCompat<
                <Self::Provider as TransactionsProvider>::Transaction,
                Transaction = RpcTransaction<Self::NetworkTypes>,
                Error = RpcError<Self>,
            >,
        >,
{
}

impl<T> FullEthApiTypes for T where
    T: RpcNodeCore<
            Provider: TransactionsProvider<Transaction = TransactionSigned>
                          + ReceiptProvider<Receipt = reth_primitives::Receipt>,
        > + EthApiTypes<
            TransactionCompat: TransactionCompat<
                <Self::Provider as TransactionsProvider>::Transaction,
                Transaction = RpcTransaction<T::NetworkTypes>,
                Error = RpcError<T>,
            >,
        >
{
}

//! Trait for specifying `eth` network dependent API types.

use std::{error::Error, fmt};

use alloy_network::{AnyNetwork, Network};
use alloy_rpc_types::Block;
use reth_rpc_eth_types::EthApiError;
use reth_rpc_types_compat::TransactionCompat;

use crate::{AsEthApiError, FromEthApiError, FromEvmError};

/// Network specific `eth` API types.
pub trait EthApiTypes: Send + Sync + Clone {
    /// Extension of [`EthApiError`], with network specific errors.
    type Error: Into<jsonrpsee_types::error::ErrorObject<'static>>
        + FromEthApiError
        + AsEthApiError
        + FromEvmError
        + Error
        + Send
        + Sync;
    /// Blockchain primitive types, specific to network, e.g. block and transaction.
    type NetworkTypes: Network<HeaderResponse = alloy_rpc_types::Header>;
    /// Conversion methods for transaction RPC type.
    type TransactionCompat: Send + Sync + Clone + fmt::Debug;
}

impl EthApiTypes for () {
    type Error = EthApiError;
    type NetworkTypes = AnyNetwork;
    type TransactionCompat = ();
}

/// Adapter for network specific transaction type.
pub type RpcTransaction<T> = <T as Network>::TransactionResponse;

/// Adapter for network specific block type.
pub type RpcBlock<T> = Block<RpcTransaction<T>, <T as Network>::HeaderResponse>;

/// Adapter for network specific receipt type.
pub type RpcReceipt<T> = <T as Network>::ReceiptResponse;

/// Helper trait holds necessary trait bounds on [`EthApiTypes`] to implement `eth` API.
pub trait FullEthApiTypes:
    EthApiTypes<TransactionCompat: TransactionCompat<Transaction = RpcTransaction<Self::NetworkTypes>>>
{
}

impl<T> FullEthApiTypes for T where
    T: EthApiTypes<
        TransactionCompat: TransactionCompat<Transaction = RpcTransaction<T::NetworkTypes>>,
    >
{
}

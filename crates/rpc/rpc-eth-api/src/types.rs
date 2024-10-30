//! Trait for specifying `eth` network dependent API types.

use std::{
    error::Error,
    fmt::{self, Debug},
};

use alloy_network::Network;
use alloy_rpc_types::Block;
use jsonrpsee::core::{DeserializeOwned, Serialize};
use reth_rpc_types_compat::TransactionCompat;

use crate::{AsEthApiError, FromEthApiError, FromEvmError};

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
    type NetworkTypes: Network<
        Header: Send
                    + Sync
                    + Clone
                    + Debug
                    + Unpin
                    + From<alloy_consensus::Header>
                    + Serialize
                    + DeserializeOwned
                    + 'static,
        HeaderResponse = alloy_rpc_types::Header<<Self::NetworkTypes as Network>::Header>,
    >;
    /// Conversion methods for transaction RPC type.
    type TransactionCompat: Send + Sync + Clone + fmt::Debug;

    /// Returns reference to transaction response builder.
    fn tx_resp_builder(&self) -> &Self::TransactionCompat;
}

/// Adapter for network specific transaction type.
pub type RpcTransaction<T> = <T as Network>::TransactionResponse;

/// Adapter for network specific header type.
pub type RpcHeader<T> = <T as Network>::HeaderResponse;

/// Adapter for network specific block type.
pub type RpcBlock<T> = Block<RpcTransaction<T>, RpcHeader<T>>;

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

//! Trait for specifying `eth` network dependent API types.

use std::error::Error;

use alloy_network::{AnyNetwork, Network};
use reth_rpc_eth_types::EthApiError;
use reth_rpc_types::{AnyTransactionReceipt, Block, Transaction, WithOtherFields};

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
    // todo: remove restriction [`reth_rpc_types::Transaction`]
    // todo: remove restriction [`reth_rpc_types::AnyTransactionReceipt`]
    type NetworkTypes: Network<
        TransactionResponse = WithOtherFields<Transaction>,
        HeaderResponse = reth_rpc_types::Header,
        ReceiptResponse = AnyTransactionReceipt,
    >;
}

impl EthApiTypes for () {
    type Error = EthApiError;
    type NetworkTypes = AnyNetwork;
}

/// Adapter for network specific transaction type.
pub type RpcTransaction<T> = <T as Network>::TransactionResponse;

/// Adapter for network specific block type.
pub type RpcBlock<T> = Block<RpcTransaction<T>, <T as Network>::HeaderResponse>;

/// Adapter for network specific receipt type.
pub type RpcReceipt<T> = <T as Network>::ReceiptResponse;

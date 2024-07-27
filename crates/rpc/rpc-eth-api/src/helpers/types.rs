//! Trait for specifying `eth` API types that may be network dependent.

use std::error::Error;

use alloy_network::Network;
use reth_rpc_types::Rich;

use crate::{AsEthApiError, FromEthApiError, FromEvmError};

/// Network specific `eth` API types.
pub trait EthApiTypes: Send + Sync {
    /// Extension of [`EthApiError`](reth_rpc_eth_types::EthApiError), with network specific errors.
    type Error: Into<jsonrpsee_types::error::ErrorObject<'static>>
        + FromEthApiError
        + AsEthApiError
        + FromEvmError
        + Error
        + Send
        + Sync;
    /// Blockchain data types, specific to network, e.g. block and transaction.
    type NetworkTypes: Network;
}

/// Adapter for network specific transaction type.
pub type Transaction<T> = <<T as EthApiTypes>::NetworkTypes as Network>::TransactionResponse;

/// Adapter for network specific block type.
pub type Block<T> = Rich<reth_rpc_types::Block<Transaction<T>>>;

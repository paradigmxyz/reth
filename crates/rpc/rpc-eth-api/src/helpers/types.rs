//! Trait for specifying `eth` API types that may be network dependent.

use std::{error::Error, fmt};

use alloy_network::{AnyNetwork, Network};
use reth_rpc_eth_types::EthApiError;
use reth_rpc_types::Rich;

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
    type NetworkTypes: Network;
    /// Conversion methods for transaction RPC type.
    type TransactionCompat: Send + Sync + Clone + fmt::Debug;
}

impl EthApiTypes for () {
    type Error = EthApiError;
    type NetworkTypes = AnyNetwork;
    type TransactionCompat = ();
}

/// Adapter for network specific transaction type.
pub type Transaction<T> = <T as Network>::TransactionResponse;

/// Adapter for network specific block type.
pub type Block<T> = Rich<reth_rpc_types::Block<Transaction<T>>>;

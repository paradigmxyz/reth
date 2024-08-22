//! Trait for specifying `eth` network dependent API types.

use std::error::Error;

use alloy_network::{Ethereum, Network};
use reth_rpc_eth_types::EthApiError;
use reth_rpc_types::{Block, Transaction};

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
    // todo: remove restriction `reth_rpc_types::Transaction`
    type NetworkTypes: Network<TransactionResponse = Transaction>;
}

impl EthApiTypes for () {
    type Error = EthApiError;
    type NetworkTypes = Ethereum;
}

/// Adapter for network specific transaction type.
pub type RpcTransaction<T> = <T as Network>::TransactionResponse;

/// Adapter for network specific block type.
pub type RpcBlock<T> = Block<RpcTransaction<T>>;

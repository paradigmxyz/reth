//! Trait for specifying `eth` network dependent API types.

use crate::{AsEthApiError, FromEthApiError, RpcNodeCore};
use alloy_rpc_types_eth::{Block, TransactionRequest};
use reth_chain_state::CanonStateSubscriptions;
use reth_rpc_convert::RpcConvert;
use reth_storage_api::{ProviderTx, ReceiptProvider, TransactionsProvider};
use reth_transaction_pool::{PoolTransaction, TransactionPool};
use std::{
    error::Error,
    fmt::{self},
};

pub use reth_rpc_convert::{RpcTransaction, RpcTxReq, RpcTypes};

/// Network specific `eth` API types.
///
/// This trait defines the network specific rpc types and helpers required for the `eth_` and
/// adjacent endpoints. `NetworkTypes` is [`alloy_network::Network`] as defined by the alloy crate,
/// see also [`alloy_network::Ethereum`].
///
/// This type is stateful so that it can provide additional context if necessary, e.g. populating
/// receipts with additional data.
pub trait EthApiTypes: Send + Sync + Clone {
    /// Extension of [`FromEthApiError`], with network specific errors.
    type Error: Into<jsonrpsee_types::error::ErrorObject<'static>>
        + FromEthApiError
        + AsEthApiError
        + Error
        + Send
        + Sync;
    /// Blockchain primitive types, specific to network, e.g. block and transaction.
    type NetworkTypes: RpcTypes;
    /// Conversion methods for transaction RPC type.
    type RpcConvert: Send + Sync + Clone + fmt::Debug;

    /// Returns reference to transaction response builder.
    fn tx_resp_builder(&self) -> &Self::RpcConvert;
}

/// Adapter for network specific block type.
pub type RpcBlock<T> = Block<RpcTransaction<T>, RpcHeader<T>>;

/// Adapter for network specific receipt type.
pub type RpcReceipt<T> = <T as RpcTypes>::Receipt;

/// Adapter for network specific header type.
pub type RpcHeader<T> = <T as RpcTypes>::Header;

/// Adapter for network specific error type.
pub type RpcError<T> = <T as EthApiTypes>::Error;

/// Helper trait holds necessary trait bounds on [`EthApiTypes`] to implement `eth` API.
pub trait FullEthApiTypes
where
    Self: RpcNodeCore<
            Provider: TransactionsProvider + ReceiptProvider + CanonStateSubscriptions,
            Pool: TransactionPool<
                Transaction: PoolTransaction<Consensus = ProviderTx<Self::Provider>>,
            >,
        > + EthApiTypes<
            RpcConvert: RpcConvert<
                Primitives = <Self as RpcNodeCore>::Primitives,
                Network = Self::NetworkTypes,
                Error = RpcError<Self>,
            >,
            NetworkTypes: RpcTypes<TransactionRequest: From<TransactionRequest>>,
        >,
{
}

impl<T> FullEthApiTypes for T where
    T: RpcNodeCore<
            Provider: TransactionsProvider + ReceiptProvider + CanonStateSubscriptions,
            Pool: TransactionPool<
                Transaction: PoolTransaction<Consensus = ProviderTx<Self::Provider>>,
            >,
        > + EthApiTypes<
            RpcConvert: RpcConvert<
                Primitives = <Self as RpcNodeCore>::Primitives,
                Network = Self::NetworkTypes,
                Error = RpcError<T>,
            >,
            NetworkTypes: RpcTypes<TransactionRequest: From<TransactionRequest>>,
        >
{
}

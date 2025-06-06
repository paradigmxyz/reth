//! Compatibility functions for rpc `Transaction` type.

use alloy_consensus::{
    error::ValueError, transaction::Recovered, EthereumTxEnvelope, TxEip4844, TxEip4844Variant,
};
use alloy_network::Network;
use alloy_primitives::Address;
use alloy_rpc_types_eth::{request::TransactionRequest, Transaction, TransactionInfo};
use core::error;
use reth_primitives_traits::{NodePrimitives, SignedTransaction, TxTy};
use serde::{Deserialize, Serialize};
use std::{fmt::Debug, marker::PhantomData};
use thiserror::Error;

/// Builds RPC transaction w.r.t. network.
pub trait TransactionCompat: Send + Sync + Unpin + Clone + Debug {
    /// The lower layer consensus types to convert from.
    type Primitives: NodePrimitives;

    /// RPC transaction response type.
    type Transaction: Serialize + for<'de> Deserialize<'de> + Send + Sync + Unpin + Clone + Debug;

    /// RPC transaction error type.
    type Error: error::Error + Into<jsonrpsee_types::ErrorObject<'static>>;

    /// Wrapper for `fill()` with default `TransactionInfo`
    /// Create a new rpc transaction result for a _pending_ signed transaction, setting block
    /// environment related fields to `None`.
    fn fill_pending(
        &self,
        tx: Recovered<TxTy<Self::Primitives>>,
    ) -> Result<Self::Transaction, Self::Error> {
        self.fill(tx, TransactionInfo::default())
    }

    /// Create a new rpc transaction result for a mined transaction, using the given block hash,
    /// number, and tx index fields to populate the corresponding fields in the rpc result.
    ///
    /// The block hash, number, and tx index fields should be from the original block where the
    /// transaction was mined.
    fn fill(
        &self,
        tx: Recovered<TxTy<Self::Primitives>>,
        tx_inf: TransactionInfo,
    ) -> Result<Self::Transaction, Self::Error>;

    /// Builds a fake transaction from a transaction request for inclusion into block built in
    /// `eth_simulateV1`.
    fn build_simulate_v1_transaction(
        &self,
        request: TransactionRequest,
    ) -> Result<TxTy<Self::Primitives>, Self::Error>;
}

/// Converts `self` into `T`.
///
/// Should create an RPC transaction response object based on a consensus transaction, its signer
/// [`Address`] and an additional context carried as [`TransactionInfo`].
pub trait IntoRpcTx<T> {
    /// Performs the conversion.
    fn into_rpc_tx(self, signer: Address, tx_info: TransactionInfo) -> T;
}

/// Converts `self` into `T`.
///
/// Should create a fake transaction for simulation using [`TransactionRequest`].
pub trait TryIntoSimTx<T>
where
    Self: Sized,
{
    /// Performs the conversion.
    ///
    /// Should return a signed typed transaction envelope for the [`eth_simulateV1`] endpoint with a
    /// dummy signature or an error if [required fields] are missing.
    ///
    /// [`eth_simulateV1`]: <https://github.com/ethereum/execution-apis/pull/484>
    /// [required fields]: TransactionRequest::buildable_type
    fn try_into_sim_tx(self) -> Result<T, ValueError<Self>>;
}

impl IntoRpcTx<Transaction> for EthereumTxEnvelope<TxEip4844> {
    fn into_rpc_tx(self, signer: Address, tx_info: TransactionInfo) -> Transaction {
        Transaction::from_transaction(
            self.with_signer(signer).map(|v| match v {
                Self::Eip4844(v) => EthereumTxEnvelope::Eip4844(v.map(TxEip4844Variant::TxEip4844)),
                Self::Legacy(v) => EthereumTxEnvelope::Legacy(v),
                Self::Eip2930(v) => EthereumTxEnvelope::Eip2930(v),
                Self::Eip1559(v) => EthereumTxEnvelope::Eip1559(v),
                Self::Eip7702(v) => EthereumTxEnvelope::Eip7702(v),
            }),
            tx_info,
        )
    }
}

impl TryIntoSimTx<EthereumTxEnvelope<TxEip4844>> for TransactionRequest {
    fn try_into_sim_tx(self) -> Result<EthereumTxEnvelope<TxEip4844>, ValueError<Self>> {
        Self::build_typed_simulate_transaction(self)
    }
}

/// Error that occurred during conversions into RPC response.
#[derive(Debug, Clone, Error)]
pub enum CompatError {
    /// Error that happens on conversion into transaction RPC response.
    #[error("Failed to convert transaction into RPC response")]
    TransactionConversionError,
}

impl From<CompatError> for jsonrpsee_types::ErrorObject<'static> {
    fn from(_value: CompatError) -> Self {
        todo!()
    }
}

/// Generic RPC response object converter for primitives `N` and network `E`.
#[derive(Debug, Clone)]
pub struct RpcTransactionConverter<N, E>(PhantomData<(N, E)>);

impl<N, E> TransactionCompat for RpcTransactionConverter<N, E>
where
    N: NodePrimitives,
    E: Network + Unpin,
    TxTy<N>: IntoRpcTx<<E as Network>::TransactionResponse> + Clone + Debug,
    TransactionRequest: TryIntoSimTx<TxTy<N>>,
{
    type Primitives = N;
    type Transaction = <E as Network>::TransactionResponse;
    type Error = CompatError;

    fn fill(
        &self,
        tx: Recovered<TxTy<N>>,
        tx_info: TransactionInfo,
    ) -> Result<Self::Transaction, Self::Error> {
        let (tx, signer) = tx.into_parts();
        Ok(tx.into_rpc_tx(signer, tx_info))
    }

    fn build_simulate_v1_transaction(
        &self,
        request: TransactionRequest,
    ) -> Result<TxTy<N>, Self::Error> {
        request.try_into_sim_tx().map_err(|_| CompatError::TransactionConversionError)
    }
}

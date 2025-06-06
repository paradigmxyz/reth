//! Compatibility functions for rpc `Transaction` type.

use alloy_consensus::{
    error::ValueError, transaction::Recovered, EthereumTxEnvelope, SignableTransaction, TxEip4844,
    TxEip4844Variant,
};
use alloy_network::Network;
use alloy_primitives::{Address, Signature};
use alloy_rpc_types_eth::{request::TransactionRequest, Transaction, TransactionInfo};
use core::error;
use op_alloy_consensus::{
    transaction::{OpDepositInfo, OpTransactionInfo},
    OpTxEnvelope,
};
use op_alloy_rpc_types::OpTransactionRequest;
use reth_optimism_primitives::DepositReceipt;
use reth_primitives_traits::{NodePrimitives, SignedTransaction, TxTy};
use reth_storage_api::{errors::ProviderError, ReceiptProvider};
use serde::{Deserialize, Serialize};
use std::{error::Error, fmt::Debug, marker::PhantomData};
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
/// [`Address`] and an additional context.
pub trait IntoRpcTx<T> {
    /// An additional context, usually [`TransactionInfo`] in a wrapper that carries some
    /// implementation specific extra information.
    type TxInfo;

    /// Performs the conversion.
    fn into_rpc_tx(self, signer: Address, tx_info: Self::TxInfo) -> T;
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
    type TxInfo = TransactionInfo;

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

/// Adds extra context to [`TransactionInfo`].
pub trait TxInfoMapper<T> {
    /// An associated output type that carries [`TransactionInfo`] with some extra context.
    type Out;

    /// Performs the conversion.
    fn try_map(&self, tx: T, tx_info: TransactionInfo) -> Result<Self::Out, CompatError>;
}

impl<T> TxInfoMapper<&T> for () {
    type Out = TransactionInfo;

    fn try_map(&self, _tx: &T, tx_info: TransactionInfo) -> Result<Self::Out, CompatError> {
        Ok(tx_info)
    }
}

/// Creates [`OpTransactionInfo`] by adding [`OpDepositInfo`] to [`TransactionInfo`] if `tx` is a
/// deposit.
pub fn try_into_op_tx_info<T: ReceiptProvider<Receipt: DepositReceipt>>(
    provider: &T,
    tx: &OpTxEnvelope,
    tx_info: TransactionInfo,
) -> Result<OpTransactionInfo, CompatError> {
    let deposit_meta = if tx.is_deposit() {
        provider.receipt_by_hash(tx.tx_hash())?.and_then(|receipt| {
            receipt.as_deposit_receipt().map(|receipt| OpDepositInfo {
                deposit_receipt_version: receipt.deposit_receipt_version,
                deposit_nonce: receipt.deposit_nonce,
            })
        })
    } else {
        None
    }
    .unwrap_or_default();

    Ok(OpTransactionInfo::new(tx_info, deposit_meta))
}

impl IntoRpcTx<op_alloy_rpc_types::Transaction> for OpTxEnvelope {
    type TxInfo = OpTransactionInfo;

    fn into_rpc_tx(
        self,
        signer: Address,
        tx_info: OpTransactionInfo,
    ) -> op_alloy_rpc_types::Transaction {
        op_alloy_rpc_types::Transaction::from_transaction(self.with_signer(signer), tx_info)
    }
}

impl TryIntoSimTx<EthereumTxEnvelope<TxEip4844>> for TransactionRequest {
    fn try_into_sim_tx(self) -> Result<EthereumTxEnvelope<TxEip4844>, ValueError<Self>> {
        Self::build_typed_simulate_transaction(self)
    }
}

impl TryIntoSimTx<OpTxEnvelope> for TransactionRequest {
    fn try_into_sim_tx(self) -> Result<OpTxEnvelope, ValueError<Self>> {
        let request: OpTransactionRequest = self.into();
        let tx = request.build_typed_tx().map_err(|request| {
            ValueError::new(request.as_ref().clone(), "Required fields missing")
        })?;

        // Create an empty signature for the transaction.
        let signature = Signature::new(Default::default(), Default::default(), false);

        Ok(tx.into_signed(signature).into())
    }
}

/// Error that occurred during conversions into RPC response.
#[derive(Debug, Clone, Error)]
pub enum CompatError {
    /// Conversion into transaction RPC response failed.
    #[error("Failed to convert transaction into RPC response")]
    TransactionConversionError,
    /// Storage access failed.
    #[error(transparent)]
    Provider(#[from] ProviderError),
}

/// Generic RPC response object converter for primitives `N` and network `E`.
#[derive(Debug)]
pub struct RpcTransactionConverter<N, E, Err, Map = ()> {
    phantom: PhantomData<(N, E, Err)>,
    mapper: Map,
}

impl<N, E, Err> RpcTransactionConverter<N, E, Err, ()> {
    /// Creates a new [`RpcTransactionConverter`] with the default mapper.
    pub const fn new() -> Self {
        Self::with_mapper(())
    }
}

impl<N, E, Err, Map> RpcTransactionConverter<N, E, Err, Map> {
    /// Creates a new [`RpcTransactionConverter`] with `mapper`.
    pub const fn with_mapper(mapper: Map) -> Self {
        Self { phantom: PhantomData, mapper }
    }
}

impl<N, E, Err, Map: Clone> Clone for RpcTransactionConverter<N, E, Err, Map> {
    fn clone(&self) -> Self {
        Self::with_mapper(self.mapper.clone())
    }
}

impl<N, E, Err> Default for RpcTransactionConverter<N, E, Err> {
    fn default() -> Self {
        Self::new()
    }
}

impl<N, E, Err, Map> TransactionCompat for RpcTransactionConverter<N, E, Err, Map>
where
    N: NodePrimitives,
    E: Network + Unpin,
    TxTy<N>: IntoRpcTx<<E as Network>::TransactionResponse> + Clone + Debug,
    TransactionRequest: TryIntoSimTx<TxTy<N>>,
    Err: From<CompatError>
        + Error
        + Unpin
        + Sync
        + Send
        + Into<jsonrpsee_types::ErrorObject<'static>>,
    Map: for<'a> TxInfoMapper<
            &'a TxTy<N>,
            Out = <TxTy<N> as IntoRpcTx<<E as Network>::TransactionResponse>>::TxInfo,
        > + Clone
        + Debug
        + Unpin
        + Send
        + Sync,
{
    type Primitives = N;
    type Transaction = <E as Network>::TransactionResponse;
    type Error = Err;

    fn fill(
        &self,
        tx: Recovered<TxTy<N>>,
        tx_info: TransactionInfo,
    ) -> Result<Self::Transaction, Self::Error> {
        let (tx, signer) = tx.into_parts();
        let tx_info = self.mapper.try_map(&tx, tx_info)?;

        Ok(tx.into_rpc_tx(signer, tx_info))
    }

    fn build_simulate_v1_transaction(
        &self,
        request: TransactionRequest,
    ) -> Result<TxTy<N>, Self::Error> {
        Ok(request.try_into_sim_tx().map_err(|_| CompatError::TransactionConversionError)?)
    }
}

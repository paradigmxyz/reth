//! Compatibility functions for rpc `Transaction` type.

use alloy_consensus::{
    error::ValueError, transaction::Recovered, EthereumTxEnvelope, SignableTransaction, TxEip4844,
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
use std::{convert::Infallible, error::Error, fmt::Debug, marker::PhantomData};
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
        Transaction::from_transaction(self.with_signer(signer).convert(), tx_info)
    }
}

/// Adds extra context to [`TransactionInfo`].
pub trait TxInfoMapper<T> {
    /// An associated output type that carries [`TransactionInfo`] with some extra context.
    type Out;
    /// An associated error that can occur during the mapping.
    type Err;

    /// Performs the conversion.
    fn try_map(&self, tx: T, tx_info: TransactionInfo) -> Result<Self::Out, Self::Err>;
}

impl<T> TxInfoMapper<&T> for () {
    type Out = TransactionInfo;
    type Err = Infallible;

    fn try_map(&self, _tx: &T, tx_info: TransactionInfo) -> Result<Self::Out, Self::Err> {
        Ok(tx_info)
    }
}

/// Creates [`OpTransactionInfo`] by adding [`OpDepositInfo`] to [`TransactionInfo`] if `tx` is a
/// deposit.
pub fn try_into_op_tx_info<T: ReceiptProvider<Receipt: DepositReceipt>>(
    provider: &T,
    tx: &OpTxEnvelope,
    tx_info: TransactionInfo,
) -> Result<OpTransactionInfo, ProviderError> {
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

/// Conversion into transaction RPC response failed.
#[derive(Debug, Clone, Error)]
#[error("Failed to convert transaction into RPC response: {0}")]
pub struct TransactionConversionError(String);

/// Generic RPC response object converter for primitives `N` and network `E`.
#[derive(Debug)]
pub struct RpcConverter<N, E, Err, Map = ()> {
    phantom: PhantomData<(N, E, Err)>,
    mapper: Map,
}

impl<N, E, Err> RpcConverter<N, E, Err, ()> {
    /// Creates a new [`RpcConverter`] with the default mapper.
    pub const fn new() -> Self {
        Self::with_mapper(())
    }
}

impl<N, E, Err, Map> RpcConverter<N, E, Err, Map> {
    /// Creates a new [`RpcConverter`] with `mapper`.
    pub const fn with_mapper(mapper: Map) -> Self {
        Self { phantom: PhantomData, mapper }
    }

    /// Converts the generic types.
    pub fn convert<N2, E2, Err2>(self) -> RpcConverter<N2, E2, Err2, Map> {
        RpcConverter::with_mapper(self.mapper)
    }

    /// Swaps the inner `mapper`.
    pub fn map<Map2>(self, mapper: Map2) -> RpcConverter<N, E, Err, Map2> {
        RpcConverter::with_mapper(mapper)
    }

    /// Converts the generic types and swaps the inner `mapper`.
    pub fn convert_map<N2, E2, Err2, Map2>(self, mapper: Map2) -> RpcConverter<N2, E2, Err2, Map2> {
        self.convert().map(mapper)
    }
}

impl<N, E, Err, Map: Clone> Clone for RpcConverter<N, E, Err, Map> {
    fn clone(&self) -> Self {
        Self::with_mapper(self.mapper.clone())
    }
}

impl<N, E, Err> Default for RpcConverter<N, E, Err> {
    fn default() -> Self {
        Self::new()
    }
}

impl<N, E, Err, Map> TransactionCompat for RpcConverter<N, E, Err, Map>
where
    N: NodePrimitives,
    E: Network + Unpin,
    TxTy<N>: IntoRpcTx<<E as Network>::TransactionResponse> + Clone + Debug,
    TransactionRequest: TryIntoSimTx<TxTy<N>>,
    Err: From<TransactionConversionError>
        + for<'a> From<<Map as TxInfoMapper<&'a TxTy<N>>>::Err>
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
        Ok(request.try_into_sim_tx().map_err(|e| TransactionConversionError(e.to_string()))?)
    }
}

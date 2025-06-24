//! Compatibility functions for rpc `Transaction` type.

use crate::{
    fees::{CallFees, CallFeesError},
    RpcTransaction, RpcTxReq, RpcTypes,
};
use alloy_consensus::{error::ValueError, transaction::Recovered, EthereumTxEnvelope, TxEip4844};
use alloy_primitives::{Address, TxKind, U256};
use alloy_rpc_types_eth::{
    request::{TransactionInputError, TransactionRequest},
    Transaction, TransactionInfo,
};
use core::error;
use reth_evm::{
    revm::context_interface::{either::Either, Block},
    ConfigureEvm, TxEnvFor,
};
use reth_primitives_traits::{NodePrimitives, TxTy};
use revm_context::{BlockEnv, CfgEnv, TxEnv};
use std::{convert::Infallible, error::Error, fmt::Debug, marker::PhantomData};
use thiserror::Error;

/// Responsible for the conversions from and into RPC requests and responses.
///
/// The JSON-RPC schema and the Node primitives are configurable using the [`RpcConvert::Network`]
/// and [`RpcConvert::Primitives`] associated types respectively.
///
/// A generic implementation [`RpcConverter`] should be preferred over a manual implementation. As
/// long as its trait bound requirements are met, the implementation is created automatically and
/// can be used in RPC method handlers for all the conversions.
pub trait RpcConvert: Send + Sync + Unpin + Clone + Debug {
    /// Associated lower layer consensus types to convert from and into types of [`Self::Network`].
    type Primitives: NodePrimitives;

    /// Associated upper layer JSON-RPC API network requests and responses to convert from and into
    /// types of [`Self::Primitives`].
    type Network: RpcTypes + Send + Sync + Unpin + Clone + Debug;

    /// A set of variables for executing a transaction.
    type TxEnv;

    /// An associated RPC conversion error.
    type Error: error::Error + Into<jsonrpsee_types::ErrorObject<'static>>;

    /// Wrapper for `fill()` with default `TransactionInfo`
    /// Create a new rpc transaction result for a _pending_ signed transaction, setting block
    /// environment related fields to `None`.
    fn fill_pending(
        &self,
        tx: Recovered<TxTy<Self::Primitives>>,
    ) -> Result<RpcTransaction<Self::Network>, Self::Error> {
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
    ) -> Result<RpcTransaction<Self::Network>, Self::Error>;

    /// Builds a fake transaction from a transaction request for inclusion into block built in
    /// `eth_simulateV1`.
    fn build_simulate_v1_transaction(
        &self,
        request: RpcTxReq<Self::Network>,
    ) -> Result<TxTy<Self::Primitives>, Self::Error>;

    /// Creates a transaction environment for execution based on `request` with corresponding
    /// `cfg_env` and `block_env`.
    fn tx_env<Spec>(
        &self,
        request: RpcTxReq<Self::Network>,
        cfg_env: &CfgEnv<Spec>,
        block_env: &BlockEnv,
    ) -> Result<Self::TxEnv, Self::Error>;
}

/// Converts `self` into `T`. The opposite of [`FromConsensusTx`].
///
/// Should create an RPC transaction response object based on a consensus transaction, its signer
/// [`Address`] and an additional context [`IntoRpcTx::TxInfo`].
///
/// Avoid implementing [`IntoRpcTx`] and use [`FromConsensusTx`] instead. Implementing it
/// automatically provides an implementation of [`IntoRpcTx`] thanks to the blanket implementation
/// in this crate.
///
/// Prefer using [`IntoRpcTx`] over [`FromConsensusTx`] when specifying trait bounds on a generic
/// function to ensure that types that only implement [`IntoRpcTx`] can be used as well.
pub trait IntoRpcTx<T> {
    /// An additional context, usually [`TransactionInfo`] in a wrapper that carries some
    /// implementation specific extra information.
    type TxInfo;

    /// Performs the conversion consuming `self` with `signer` and `tx_info`. See [`IntoRpcTx`]
    /// for details.
    fn into_rpc_tx(self, signer: Address, tx_info: Self::TxInfo) -> T;
}

/// Converts `T` into `self`. It is reciprocal of [`IntoRpcTx`].
///
/// Should create an RPC transaction response object based on a consensus transaction, its signer
/// [`Address`] and an additional context [`FromConsensusTx::TxInfo`].
///
/// Prefer implementing [`FromConsensusTx`] over [`IntoRpcTx`] because it automatically provides an
/// implementation of [`IntoRpcTx`] thanks to the blanket implementation in this crate.
///
/// Prefer using [`IntoRpcTx`] over using [`FromConsensusTx`] when specifying trait bounds on a
/// generic function. This way, types that directly implement [`IntoRpcTx`] can be used as arguments
/// as well.
pub trait FromConsensusTx<T> {
    /// An additional context, usually [`TransactionInfo`] in a wrapper that carries some
    /// implementation specific extra information.
    type TxInfo;

    /// Performs the conversion consuming `tx` with `signer` and `tx_info`. See [`FromConsensusTx`]
    /// for details.
    fn from_consensus_tx(tx: T, signer: Address, tx_info: Self::TxInfo) -> Self;
}

impl<TxIn: alloy_consensus::Transaction, T: alloy_consensus::Transaction + From<TxIn>>
    FromConsensusTx<TxIn> for Transaction<T>
{
    type TxInfo = TransactionInfo;

    fn from_consensus_tx(tx: TxIn, signer: Address, tx_info: Self::TxInfo) -> Self {
        Self::from_transaction(Recovered::new_unchecked(tx.into(), signer), tx_info)
    }
}

impl<ConsensusTx, RpcTx> IntoRpcTx<RpcTx> for ConsensusTx
where
    ConsensusTx: alloy_consensus::Transaction,
    RpcTx: FromConsensusTx<Self>,
{
    type TxInfo = RpcTx::TxInfo;

    fn into_rpc_tx(self, signer: Address, tx_info: Self::TxInfo) -> RpcTx {
        RpcTx::from_consensus_tx(self, signer, tx_info)
    }
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

impl TryIntoSimTx<EthereumTxEnvelope<TxEip4844>> for TransactionRequest {
    fn try_into_sim_tx(self) -> Result<EthereumTxEnvelope<TxEip4844>, ValueError<Self>> {
        Self::build_typed_simulate_transaction(self)
    }
}

/// Converts `self` into `T`.
///
/// Should create an executable transaction environment using [`TransactionRequest`].
pub trait TryIntoTxEnv<T> {
    /// An associated error that can occur during the conversion.
    type Err;

    /// Performs the conversion.
    fn try_into_tx_env<Spec>(
        self,
        cfg_env: &CfgEnv<Spec>,
        block_env: &BlockEnv,
    ) -> Result<T, Self::Err>;
}

/// An Ethereum specific transaction environment error than can occur during conversion from
/// [`TransactionRequest`].
#[derive(Debug, Error)]
pub enum EthTxEnvError {
    /// Error while decoding or validating transaction request fees.
    #[error(transparent)]
    CallFees(#[from] CallFeesError),
    /// Both data and input fields are set and not equal.
    #[error(transparent)]
    Input(#[from] TransactionInputError),
}

impl TryIntoTxEnv<TxEnv> for TransactionRequest {
    type Err = EthTxEnvError;

    fn try_into_tx_env<Spec>(
        self,
        cfg_env: &CfgEnv<Spec>,
        block_env: &BlockEnv,
    ) -> Result<TxEnv, Self::Err> {
        // Ensure that if versioned hashes are set, they're not empty
        if self.blob_versioned_hashes.as_ref().is_some_and(|hashes| hashes.is_empty()) {
            return Err(CallFeesError::BlobTransactionMissingBlobHashes.into())
        }

        let tx_type = self.minimal_tx_type() as u8;

        let Self {
            from,
            to,
            gas_price,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            gas,
            value,
            input,
            nonce,
            access_list,
            chain_id,
            blob_versioned_hashes,
            max_fee_per_blob_gas,
            authorization_list,
            transaction_type: _,
            sidecar: _,
        } = self;

        let CallFees { max_priority_fee_per_gas, gas_price, max_fee_per_blob_gas } =
            CallFees::ensure_fees(
                gas_price.map(U256::from),
                max_fee_per_gas.map(U256::from),
                max_priority_fee_per_gas.map(U256::from),
                U256::from(block_env.basefee),
                blob_versioned_hashes.as_deref(),
                max_fee_per_blob_gas.map(U256::from),
                block_env.blob_gasprice().map(U256::from),
            )?;

        let gas_limit = gas.unwrap_or(
            // Use maximum allowed gas limit. The reason for this
            // is that both Erigon and Geth use pre-configured gas cap even if
            // it's possible to derive the gas limit from the block:
            // <https://github.com/ledgerwatch/erigon/blob/eae2d9a79cb70dbe30b3a6b79c436872e4605458/cmd/rpcdaemon/commands/trace_adhoc.go#L956
            // https://github.com/ledgerwatch/erigon/blob/eae2d9a79cb70dbe30b3a6b79c436872e4605458/eth/ethconfig/config.go#L94>
            block_env.gas_limit,
        );

        let chain_id = chain_id.unwrap_or(cfg_env.chain_id);

        let caller = from.unwrap_or_default();

        let nonce = nonce.unwrap_or_default();

        let env = TxEnv {
            tx_type,
            gas_limit,
            nonce,
            caller,
            gas_price: gas_price.saturating_to(),
            gas_priority_fee: max_priority_fee_per_gas.map(|v| v.saturating_to()),
            kind: to.unwrap_or(TxKind::Create),
            value: value.unwrap_or_default(),
            data: input.try_into_unique_input().map_err(EthTxEnvError::from)?.unwrap_or_default(),
            chain_id: Some(chain_id),
            access_list: access_list.unwrap_or_default(),
            // EIP-4844 fields
            blob_hashes: blob_versioned_hashes.unwrap_or_default(),
            max_fee_per_blob_gas: max_fee_per_blob_gas
                .map(|v| v.saturating_to())
                .unwrap_or_default(),
            // EIP-7702 fields
            authorization_list: authorization_list
                .unwrap_or_default()
                .into_iter()
                .map(Either::Left)
                .collect(),
        };

        Ok(env)
    }
}

/// Conversion into transaction RPC response failed.
#[derive(Debug, Clone, Error)]
#[error("Failed to convert transaction into RPC response: {0}")]
pub struct TransactionConversionError(String);

/// Generic RPC response object converter for `Evm` and network `E`.
///
/// The main purpose of this struct is to provide an implementation of [`RpcConvert`] for generic
/// associated types. This struct can then be used for conversions in RPC method handlers.
///
/// An [`RpcConvert`] implementation is generated if the following traits are implemented for the
/// network and EVM associated primitives:
/// * [`FromConsensusTx`]: from signed transaction into RPC response object.
/// * [`TryIntoSimTx`]: from RPC transaction request into a simulated transaction.
/// * [`TryIntoTxEnv`]: from RPC transaction request into an executable transaction.
/// * [`TxInfoMapper`]: from [`TransactionInfo`] into [`FromConsensusTx::TxInfo`]. Should be
///   implemented for a dedicated struct that is assigned to `Map`. If [`FromConsensusTx::TxInfo`]
///   is [`TransactionInfo`] then `()` can be used as `Map` which trivially passes over the input
///   object.
#[derive(Debug)]
pub struct RpcConverter<E, Evm, Err, Map = ()> {
    phantom: PhantomData<(E, Evm, Err)>,
    mapper: Map,
}

impl<E, Evm, Err> RpcConverter<E, Evm, Err, ()> {
    /// Creates a new [`RpcConverter`] with the default mapper.
    pub const fn new() -> Self {
        Self::with_mapper(())
    }
}

impl<E, Evm, Err, Map> RpcConverter<E, Evm, Err, Map> {
    /// Creates a new [`RpcConverter`] with `mapper`.
    pub const fn with_mapper(mapper: Map) -> Self {
        Self { phantom: PhantomData, mapper }
    }

    /// Converts the generic types.
    pub fn convert<E2, Evm2, Err2>(self) -> RpcConverter<E2, Evm2, Err2, Map> {
        RpcConverter::with_mapper(self.mapper)
    }

    /// Swaps the inner `mapper`.
    pub fn map<Map2>(self, mapper: Map2) -> RpcConverter<E, Evm, Err, Map2> {
        RpcConverter::with_mapper(mapper)
    }

    /// Converts the generic types and swaps the inner `mapper`.
    pub fn convert_map<E2, Evm2, Err2, Map2>(
        self,
        mapper: Map2,
    ) -> RpcConverter<E2, Evm2, Err2, Map2> {
        self.convert().map(mapper)
    }
}

impl<E, Evm, Err, Map: Clone> Clone for RpcConverter<E, Evm, Err, Map> {
    fn clone(&self) -> Self {
        Self::with_mapper(self.mapper.clone())
    }
}

impl<E, Evm, Err> Default for RpcConverter<E, Evm, Err> {
    fn default() -> Self {
        Self::new()
    }
}

impl<N, E, Evm, Err, Map> RpcConvert for RpcConverter<E, Evm, Err, Map>
where
    N: NodePrimitives,
    E: RpcTypes + Send + Sync + Unpin + Clone + Debug,
    Evm: ConfigureEvm<Primitives = N>,
    TxTy<N>: IntoRpcTx<E::TransactionResponse> + Clone + Debug,
    RpcTxReq<E>: TryIntoSimTx<TxTy<N>> + TryIntoTxEnv<TxEnvFor<Evm>>,
    Err: From<TransactionConversionError>
        + From<<RpcTxReq<E> as TryIntoTxEnv<TxEnvFor<Evm>>>::Err>
        + for<'a> From<<Map as TxInfoMapper<&'a TxTy<N>>>::Err>
        + Error
        + Unpin
        + Sync
        + Send
        + Into<jsonrpsee_types::ErrorObject<'static>>,
    Map: for<'a> TxInfoMapper<
            &'a TxTy<N>,
            Out = <TxTy<N> as IntoRpcTx<E::TransactionResponse>>::TxInfo,
        > + Clone
        + Debug
        + Unpin
        + Send
        + Sync,
{
    type Primitives = N;
    type Network = E;
    type TxEnv = TxEnvFor<Evm>;
    type Error = Err;

    fn fill(
        &self,
        tx: Recovered<TxTy<N>>,
        tx_info: TransactionInfo,
    ) -> Result<E::TransactionResponse, Self::Error> {
        let (tx, signer) = tx.into_parts();
        let tx_info = self.mapper.try_map(&tx, tx_info)?;

        Ok(tx.into_rpc_tx(signer, tx_info))
    }

    fn build_simulate_v1_transaction(&self, request: RpcTxReq<E>) -> Result<TxTy<N>, Self::Error> {
        Ok(request.try_into_sim_tx().map_err(|e| TransactionConversionError(e.to_string()))?)
    }

    fn tx_env<Spec>(
        &self,
        request: RpcTxReq<E>,
        cfg_env: &CfgEnv<Spec>,
        block_env: &BlockEnv,
    ) -> Result<Self::TxEnv, Self::Error> {
        Ok(request.try_into_tx_env(cfg_env, block_env)?)
    }
}

/// Optimism specific RPC transaction compatibility implementations.
#[cfg(feature = "op")]
pub mod op {
    use super::*;
    use alloy_consensus::SignableTransaction;
    use alloy_primitives::{Address, Bytes, Signature};
    use op_alloy_consensus::{
        transaction::{OpDepositInfo, OpTransactionInfo},
        OpTxEnvelope,
    };
    use op_alloy_rpc_types::OpTransactionRequest;
    use op_revm::OpTransaction;
    use reth_optimism_primitives::DepositReceipt;
    use reth_storage_api::{errors::ProviderError, ReceiptProvider};

    /// Creates [`OpTransactionInfo`] by adding [`OpDepositInfo`] to [`TransactionInfo`] if `tx` is
    /// a deposit.
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

    impl<T: op_alloy_consensus::OpTransaction + alloy_consensus::Transaction> FromConsensusTx<T>
        for op_alloy_rpc_types::Transaction<T>
    {
        type TxInfo = OpTransactionInfo;

        fn from_consensus_tx(tx: T, signer: Address, tx_info: Self::TxInfo) -> Self {
            Self::from_transaction(Recovered::new_unchecked(tx, signer), tx_info)
        }
    }

    impl TryIntoSimTx<OpTxEnvelope> for OpTransactionRequest {
        fn try_into_sim_tx(self) -> Result<OpTxEnvelope, ValueError<Self>> {
            let tx = self
                .build_typed_tx()
                .map_err(|request| ValueError::new(request, "Required fields missing"))?;

            // Create an empty signature for the transaction.
            let signature = Signature::new(Default::default(), Default::default(), false);

            Ok(tx.into_signed(signature).into())
        }
    }

    impl TryIntoTxEnv<OpTransaction<TxEnv>> for OpTransactionRequest {
        type Err = EthTxEnvError;

        fn try_into_tx_env<Spec>(
            self,
            cfg_env: &CfgEnv<Spec>,
            block_env: &BlockEnv,
        ) -> Result<OpTransaction<TxEnv>, Self::Err> {
            Ok(OpTransaction {
                base: self.as_ref().clone().try_into_tx_env(cfg_env, block_env)?,
                enveloped_tx: Some(Bytes::new()),
                deposit: Default::default(),
            })
        }
    }
}

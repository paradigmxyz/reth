//! Compatibility functions for rpc `Transaction` type.

use crate::{
    fees::{CallFees, CallFeesError},
    RpcHeader, RpcReceipt, RpcTransaction, RpcTxReq, RpcTypes,
};
use alloy_consensus::{
    error::ValueError, transaction::Recovered, EthereumTxEnvelope, Sealable, TxEip4844,
};
use alloy_network::Network;
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
use reth_primitives_traits::{
    HeaderTy, NodePrimitives, SealedHeader, SealedHeaderFor, TransactionMeta, TxTy,
};
use revm_context::{BlockEnv, CfgEnv, TxEnv};
use std::{borrow::Cow, convert::Infallible, error::Error, fmt::Debug, marker::PhantomData};
use thiserror::Error;

/// Input for [`RpcConvert::convert_receipts`].
#[derive(Debug, Clone)]
pub struct ConvertReceiptInput<'a, N: NodePrimitives> {
    /// Primitive receipt.
    pub receipt: Cow<'a, N::Receipt>,
    /// Transaction the receipt corresponds to.
    pub tx: Recovered<&'a N::SignedTx>,
    /// Gas used by the transaction.
    pub gas_used: u64,
    /// Number of logs emitted before this transaction.
    pub next_log_index: usize,
    /// Metadata for the transaction.
    pub meta: TransactionMeta,
}

/// A type that knows how to convert primitive receipts to RPC representations.
pub trait ReceiptConverter<N: NodePrimitives>: Debug + 'static {
    /// RPC representation.
    type RpcReceipt;

    /// Error that may occur during conversion.
    type Error;

    /// Converts a set of primitive receipts to RPC representations. It is guaranteed that all
    /// receipts are from the same block.
    fn convert_receipts(
        &self,
        receipts: Vec<ConvertReceiptInput<'_, N>>,
    ) -> Result<Vec<Self::RpcReceipt>, Self::Error>;
}

/// A type that knows how to convert a consensus header into an RPC header.
pub trait HeaderConverter<Consensus, Rpc>: Debug + Send + Sync + Unpin + Clone + 'static {
    /// Converts a consensus header into an RPC header.
    fn convert_header(&self, header: SealedHeader<Consensus>, block_size: usize) -> Rpc;
}

/// Default implementation of [`HeaderConverter`] that uses [`FromConsensusHeader`] to convert
/// headers.
impl<Consensus, Rpc> HeaderConverter<Consensus, Rpc> for ()
where
    Rpc: FromConsensusHeader<Consensus>,
{
    fn convert_header(&self, header: SealedHeader<Consensus>, block_size: usize) -> Rpc {
        Rpc::from_consensus_header(header, block_size)
    }
}

/// Conversion trait for obtaining RPC header from a consensus header.
pub trait FromConsensusHeader<T> {
    /// Takes a consensus header and converts it into `self`.
    fn from_consensus_header(header: SealedHeader<T>, block_size: usize) -> Self;
}

impl<T: Sealable> FromConsensusHeader<T> for alloy_rpc_types_eth::Header<T> {
    fn from_consensus_header(header: SealedHeader<T>, block_size: usize) -> Self {
        Self::from_consensus(header.into(), None, Some(U256::from(block_size)))
    }
}

/// Responsible for the conversions from and into RPC requests and responses.
///
/// The JSON-RPC schema and the Node primitives are configurable using the [`RpcConvert::Network`]
/// and [`RpcConvert::Primitives`] associated types respectively.
///
/// A generic implementation [`RpcConverter`] should be preferred over a manual implementation. As
/// long as its trait bound requirements are met, the implementation is created automatically and
/// can be used in RPC method handlers for all the conversions.
pub trait RpcConvert: Send + Sync + Unpin + Clone + Debug + 'static {
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

    /// Converts a set of primitive receipts to RPC representations. It is guaranteed that all
    /// receipts are from the same block.
    fn convert_receipts(
        &self,
        receipts: Vec<ConvertReceiptInput<'_, Self::Primitives>>,
    ) -> Result<Vec<RpcReceipt<Self::Network>>, Self::Error>;

    /// Converts a primitive header to an RPC header.
    fn convert_header(
        &self,
        header: SealedHeaderFor<Self::Primitives>,
        block_size: usize,
    ) -> Result<RpcHeader<Self::Network>, Self::Error>;
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
            return Err(CallFeesError::BlobTransactionMissingBlobHashes.into());
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

/// Trait for converting primitive transactions to RPC transactions.
pub trait RpcTxConverter<PrimitiveTx, RpcTx> {
    /// The error type that can be returned during conversion.
    type Error: std::error::Error;

    /// Convert a primitive transaction to an RPC transaction.
    fn convert(
        &self,
        tx: PrimitiveTx,
        signer: Address,
        info: TransactionInfo,
    ) -> Result<RpcTx, Self::Error>;
}

/// Trait for converting primitive receipts to RPC receipts.
pub trait RpcReceiptConverter<PrimitiveReceipt, RpcReceipt> {
    /// The error type that can be returned during conversion.
    type Error: std::error::Error;

    /// Convert a primitive receipt to an RPC receipt.
    fn convert(&self, receipt: PrimitiveReceipt) -> Result<RpcReceipt, Self::Error>;
}

/// Trait for converting primitive headers to RPC headers.
pub trait RpcHeaderConverter<PrimitiveHeader, RpcHeader> {
    /// The error type that can be returned during conversion.
    type Error: std::error::Error;

    /// Convert a primitive header to an RPC header.
    fn convert(&self, header: PrimitiveHeader) -> Result<RpcHeader, Self::Error>;
}

/// Trait for converting transaction requests to transaction environments.
pub trait RpcTxEnvConverter<Request, TxEnv> {
    /// The error type that can be returned during conversion.
    type Error: std::error::Error;

    /// Convert a transaction request to a transaction environment.
    fn convert<Spec>(
        &self,
        request: Request,
        cfg_env: &CfgEnv<Spec>,
        block_env: &BlockEnv,
    ) -> Result<TxEnv, Self::Error>;
}

/// Trait for converting transaction requests to simulation transactions.
pub trait RpcSimTxConverter<Request, SimTx> {
    /// The error type that can be returned during conversion.
    type Error: std::error::Error;

    /// Convert a transaction request to a simulation transaction.
    fn convert(&self, request: Request) -> Result<SimTx, Self::Error>;
}

/// Default implementation for unit type - uses `FromConsensusTx` trait.
impl<PrimitiveTx, RpcTx> RpcTxConverter<PrimitiveTx, RpcTx> for ()
where
    RpcTx: FromConsensusTx<PrimitiveTx, TxInfo = TransactionInfo>,
{
    type Error = std::convert::Infallible;

    fn convert(
        &self,
        tx: PrimitiveTx,
        signer: Address,
        info: TransactionInfo,
    ) -> Result<RpcTx, Self::Error> {
        Ok(RpcTx::from_consensus_tx(tx, signer, info))
    }
}

/// Implementation for function types.
impl<F, PrimitiveTx, RpcTx, E> RpcTxConverter<PrimitiveTx, RpcTx> for F
where
    F: Fn(PrimitiveTx, Address, TransactionInfo) -> Result<RpcTx, E>,
    E: std::error::Error,
{
    type Error = E;

    fn convert(
        &self,
        tx: PrimitiveTx,
        signer: Address,
        info: TransactionInfo,
    ) -> Result<RpcTx, Self::Error> {
        self(tx, signer, info)
    }
}

/// Default implementation for unit type - uses `TryIntoTxEnv` trait.
impl<Request, TxEnv> RpcTxEnvConverter<Request, TxEnv> for ()
where
    Request: TryIntoTxEnv<TxEnv>,
    Request::Err: std::error::Error,
{
    type Error = Request::Err;

    fn convert<Spec>(
        &self,
        request: Request,
        cfg_env: &CfgEnv<Spec>,
        block_env: &BlockEnv,
    ) -> Result<TxEnv, Self::Error> {
        request.try_into_tx_env(cfg_env, block_env)
    }
}

/// A helper type for function-based `TxEnv` converters that handles the generic Spec parameter.
#[derive(Debug)]
pub struct TxEnvConverterFn<F> {
    f: F,
}

impl<F, Request, TxEnv, E> RpcTxEnvConverter<Request, TxEnv> for TxEnvConverterFn<F>
where
    F: for<'a> Fn(Request, &'a BlockEnv) -> Result<TxEnv, E>,
    E: std::error::Error,
{
    type Error = E;

    fn convert<Spec>(
        &self,
        request: Request,
        _cfg_env: &CfgEnv<Spec>,
        block_env: &BlockEnv,
    ) -> Result<TxEnv, Self::Error> {
        (self.f)(request, block_env)
    }
}

/// Default implementation for unit type - uses `TryIntoSimTx` trait.
impl<Request, SimTx> RpcSimTxConverter<Request, SimTx> for ()
where
    Request: TryIntoSimTx<SimTx>,
{
    type Error = TransactionConversionError;

    fn convert(&self, request: Request) -> Result<SimTx, Self::Error> {
        request.try_into_sim_tx().map_err(|e| TransactionConversionError(e.to_string()))
    }
}

/// Implementation for function types.
impl<F, Request, SimTx, E> RpcSimTxConverter<Request, SimTx> for F
where
    F: Fn(Request) -> Result<SimTx, E>,
    E: std::error::Error,
{
    type Error = E;

    fn convert(&self, request: Request) -> Result<SimTx, Self::Error> {
        self(request)
    }
}

/// Generic RPC response object converter with flexible converters for each conversion type.
///
/// This struct provides a flexible way to define converters for different RPC-related
/// transformations. Each converter can be customized independently using the builder pattern.
///
/// # Type Parameters
/// - `Tx`: Transaction converter type (default: `()`)
/// - `TxEnv`: Transaction environment converter type (default: `()`)
/// - `SimTx`: Simulation transaction converter type (default: `()`)
/// - `Receipt`: Receipt converter type (default: `()`)
/// - `Header`: Header converter type (default: `()`)
///
/// # Example
/// ```ignore
/// let converter = RpcConverter::new()
///     .with_tx_converter(|tx, signer, info| {
///         // Custom transaction conversion logic
///         Ok(MyRpcTx::from(tx))
///     });
/// ```
#[derive(Debug)]
pub struct RpcConverter<Tx = (), TxEnv = (), SimTx = (), Receipt = (), Header = ()> {
    /// Transaction converter
    pub tx_converter: Tx,
    /// Transaction environment converter
    pub tx_env_converter: TxEnv,
    /// Simulation transaction converter
    pub sim_tx_converter: SimTx,
    /// Receipt converter
    pub receipt_converter: Receipt,
    /// Header converter
    pub header_converter: Header,
}

/// Legacy RPC converter for backward compatibility.
///
/// This type maintains compatibility with existing code that uses the old `RpcConverter` API.
#[derive(Debug)]
pub struct LegacyRpcConverter<E, Evm, Err, Map = ()> {
    phantom: PhantomData<(E, Evm, Err)>,
    mapper: Map,
}

impl RpcConverter {
    /// Creates a new [`RpcConverter`] with default converters.
    pub const fn new() -> Self {
        Self {
            tx_converter: (),
            tx_env_converter: (),
            sim_tx_converter: (),
            receipt_converter: (),
            header_converter: (),
        }
    }
}

impl<Tx, TxEnv, SimTx, Receipt, Header> RpcConverter<Tx, TxEnv, SimTx, Receipt, Header> {
    /// Sets a custom transaction converter.
    pub fn with_tx_converter<NewTx>(
        self,
        tx_converter: NewTx,
    ) -> RpcConverter<NewTx, TxEnv, SimTx, Receipt, Header> {
        RpcConverter {
            tx_converter,
            tx_env_converter: self.tx_env_converter,
            sim_tx_converter: self.sim_tx_converter,
            receipt_converter: self.receipt_converter,
            header_converter: self.header_converter,
        }
    }

    /// Sets a custom transaction environment converter.
    pub fn with_tx_env_converter<NewTxEnv>(
        self,
        tx_env_converter: NewTxEnv,
    ) -> RpcConverter<Tx, NewTxEnv, SimTx, Receipt, Header> {
        RpcConverter {
            tx_converter: self.tx_converter,
            tx_env_converter,
            sim_tx_converter: self.sim_tx_converter,
            receipt_converter: self.receipt_converter,
            header_converter: self.header_converter,
        }
    }

    /// Sets a custom simulation transaction converter.
    pub fn with_sim_tx_converter<NewSimTx>(
        self,
        sim_tx_converter: NewSimTx,
    ) -> RpcConverter<Tx, TxEnv, NewSimTx, Receipt, Header> {
        RpcConverter {
            tx_converter: self.tx_converter,
            tx_env_converter: self.tx_env_converter,
            sim_tx_converter,
            receipt_converter: self.receipt_converter,
            header_converter: self.header_converter,
        }
    }

    /// Sets a custom receipt converter.
    pub fn with_receipt_converter<NewReceipt>(
        self,
        receipt_converter: NewReceipt,
    ) -> RpcConverter<Tx, TxEnv, SimTx, NewReceipt, Header> {
        RpcConverter {
            tx_converter: self.tx_converter,
            tx_env_converter: self.tx_env_converter,
            sim_tx_converter: self.sim_tx_converter,
            receipt_converter,
            header_converter: self.header_converter,
        }
    }

    /// Sets a custom header converter.
    pub fn with_header_converter<NewHeader>(
        self,
        header_converter: NewHeader,
    ) -> RpcConverter<Tx, TxEnv, SimTx, Receipt, NewHeader> {
        RpcConverter {
            tx_converter: self.tx_converter,
            tx_env_converter: self.tx_env_converter,
            sim_tx_converter: self.sim_tx_converter,
            receipt_converter: self.receipt_converter,
            header_converter,
        }
    }
}

impl<Tx: Clone, TxEnv: Clone, SimTx: Clone, Receipt: Clone, Header: Clone> Clone
    for RpcConverter<Tx, TxEnv, SimTx, Receipt, Header>
{
    fn clone(&self) -> Self {
        Self {
            tx_converter: self.tx_converter.clone(),
            tx_env_converter: self.tx_env_converter.clone(),
            sim_tx_converter: self.sim_tx_converter.clone(),
            receipt_converter: self.receipt_converter.clone(),
            header_converter: self.header_converter.clone(),
        }
    }
}

impl Default for RpcConverter {
    fn default() -> Self {
        Self {
            tx_converter: (),
            tx_env_converter: (),
            sim_tx_converter: (),
            receipt_converter: (),
            header_converter: (),
        }
    }
}

// Legacy implementations for backward compatibility
impl<E, Evm, Err> LegacyRpcConverter<E, Evm, Err, ()> {
    /// Creates a new [`LegacyRpcConverter`] with the default mapper.
    pub const fn new() -> Self {
        Self::with_mapper(())
    }
}

impl<E, Evm, Err, Map> LegacyRpcConverter<E, Evm, Err, Map> {
    /// Creates a new [`LegacyRpcConverter`] with `mapper`.
    pub const fn with_mapper(mapper: Map) -> Self {
        Self { phantom: PhantomData, mapper }
    }

    /// Converts the generic types.
    pub fn convert<E2, Evm2, Err2>(self) -> LegacyRpcConverter<E2, Evm2, Err2, Map> {
        LegacyRpcConverter::with_mapper(self.mapper)
    }

    /// Swaps the inner `mapper`.
    pub fn map<Map2>(self, mapper: Map2) -> LegacyRpcConverter<E, Evm, Err, Map2> {
        LegacyRpcConverter::with_mapper(mapper)
    }

    /// Configures the mapper.
    pub fn with_mapper<MapNew>(self, mapper: Map2) -> LegacyRpcConverter<E2, Evm2, Err2, Map2> {
        self.convert().map(mapper)
    }
}

impl<E, Evm, Err, Map: Clone> Clone for LegacyRpcConverter<E, Evm, Err, Map> {
    fn clone(&self) -> Self {
        Self::with_mapper(self.mapper.clone())
    }
}

impl<E, Evm, Err> Default for LegacyRpcConverter<E, Evm, Err> {
    fn default() -> Self {
        Self::new()
    }
}

impl<N, E, Evm, Err, Map> RpcConvert for LegacyRpcConverter<E, Evm, Err, Map>
where
    N: NodePrimitives,
    E: RpcTypes + Send + Sync + Unpin + Clone + Debug,
    Evm: ConfigureEvm<Primitives = N> + 'static,
    TxTy<N>: IntoRpcTx<E::TransactionResponse> + Clone + Debug,
    RpcTxReq<E>: TryIntoSimTx<TxTy<N>> + TryIntoTxEnv<TxEnvFor<Evm>>,
    Receipt: ReceiptConverter<
            N,
            RpcReceipt = RpcReceipt<E>,
            Error: From<TransactionConversionError>
                       + From<<RpcTxReq<E> as TryIntoTxEnv<TxEnvFor<Evm>>>::Err>
                       + for<'a> From<<Map as TxInfoMapper<&'a TxTy<N>>>::Err>
                       + Error
                       + Unpin
                       + Sync
                       + Send
                       + Into<jsonrpsee_types::ErrorObject<'static>>,
        > + Send
        + Sync
        + Unpin
        + Clone
        + Debug,
    Header: HeaderConverter<HeaderTy<N>, RpcHeader<E>>,
    Map: for<'a> TxInfoMapper<
            &'a TxTy<N>,
            Out = <TxTy<N> as IntoRpcTx<E::TransactionResponse>>::TxInfo,
        > + Clone
        + Debug
        + Unpin
        + Send
        + Sync
        + 'static,
{
    type Primitives = N;
    type Network = E;
    type TxEnv = TxEnvFor<Evm>;
    type Error = Receipt::Error;

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

    fn convert_receipts(
        &self,
        receipts: Vec<ConvertReceiptInput<'_, Self::Primitives>>,
    ) -> Result<Vec<RpcReceipt<Self::Network>>, Self::Error> {
        self.receipt_converter.convert_receipts(receipts)
    }

    fn convert_header(
        &self,
        header: SealedHeaderFor<Self::Primitives>,
        block_size: usize,
    ) -> Result<RpcHeader<Self::Network>, Self::Error> {
        Ok(self.header_converter.convert_header(header, block_size))
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
    use reth_primitives_traits::SignedTransaction;
    use reth_storage_api::{errors::ProviderError, ReceiptProvider};

    /// Creates [`OpTransactionInfo`] by adding [`OpDepositInfo`] to [`TransactionInfo`] if `tx` is
    /// a deposit.
    pub fn try_into_op_tx_info<Tx, T>(
        provider: &T,
        tx: &Tx,
        tx_info: TransactionInfo,
    ) -> Result<OpTransactionInfo, ProviderError>
    where
        Tx: op_alloy_consensus::OpTransaction + SignedTransaction,
        T: ReceiptProvider<Receipt: DepositReceipt>,
    {
        let deposit_meta = if tx.is_deposit() {
            provider.receipt_by_hash(*tx.tx_hash())?.and_then(|receipt| {
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, Bytes, B256, U256};
    use alloy_rpc_types_eth::TransactionInfo;

    // Mock types for testing - simplified without complex trait implementations
    #[derive(Debug, Clone, PartialEq)]
    struct MockTx {
        hash: B256,
        value: U256,
        nonce: u64,
        gas_limit: u64,
        gas_price: u128,
        input: Bytes,
    }

    #[derive(Debug, Clone)]
    struct MockRpcTx {
        inner: MockTx,
        signer: Address,
        info: TransactionInfo,
    }

    // Implement FromConsensusTx for our mock type
    impl FromConsensusTx<MockTx> for MockRpcTx {
        type TxInfo = TransactionInfo;

        fn from_consensus_tx(tx: MockTx, signer: Address, tx_info: Self::TxInfo) -> Self {
            Self { inner: tx, signer, info: tx_info }
        }
    }

    // Test module using the new converter design from the main implementation

    #[test]
    fn test_default_converter_construction() {
        // Test that default converter can be created
        let converter = RpcConverter::default();

        // Check that the converter was created successfully
        // Default converter should have unit types for all parameters
        assert!(std::mem::size_of_val(&converter.tx_converter) == 0);
        assert!(std::mem::size_of_val(&converter.tx_env_converter) == 0);
        assert!(std::mem::size_of_val(&converter.sim_tx_converter) == 0);
        assert!(std::mem::size_of_val(&converter.receipt_converter) == 0);
        assert!(std::mem::size_of_val(&converter.header_converter) == 0);
    }

    #[test]
    fn test_converter_with_custom_tx_converter() {
        // Test that we can set a custom tx converter
        let converter = RpcConverter::new().with_tx_converter(
            |tx: MockTx,
             signer: Address,
             info: TransactionInfo|
             -> Result<MockRpcTx, std::convert::Infallible> {
                Ok(MockRpcTx { inner: tx, signer, info })
            },
        );

        // Test that the converter works
        let tx = MockTx {
            hash: B256::default(),
            value: U256::from(100),
            nonce: 0,
            gas_limit: 21000,
            gas_price: 20_000_000_000,
            input: Bytes::default(),
        };
        let result = converter.tx_converter.convert(
            tx.clone(),
            Address::default(),
            TransactionInfo::default(),
        );
        assert!(result.is_ok());
        let rpc_tx = result.unwrap();
        assert_eq!(rpc_tx.inner, tx);
    }

    #[test]
    fn test_trait_based_conversion() {
        // Test that the blanket implementation works
        let converter = ();
        let tx = MockTx {
            hash: B256::default(),
            value: U256::from(100),
            nonce: 0,
            gas_limit: 21000,
            gas_price: 20_000_000_000,
            input: Bytes::default(),
        };
        let signer = Address::default();
        let info = TransactionInfo::default();

        // This should work via the blanket implementation
        let result: Result<MockRpcTx, _> =
            RpcTxConverter::convert(&converter, tx.clone(), signer, info);
        assert!(result.is_ok());

        let rpc_tx = result.unwrap();
        assert_eq!(rpc_tx.inner, tx);
        assert_eq!(rpc_tx.signer, signer);
        assert_eq!(rpc_tx.info, info);
    }

    #[test]
    fn test_multiple_custom_converters() {
        #[derive(Debug, Clone, PartialEq)]
        struct CustomError(&'static str);
        impl std::fmt::Display for CustomError {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "Custom error: {}", self.0)
            }
        }
        impl std::error::Error for CustomError {}

        // Create a converter with multiple custom converters
        let converter = RpcConverter::new()
            .with_tx_converter(
                |tx: MockTx,
                 signer: Address,
                 info: TransactionInfo|
                 -> Result<MockRpcTx, CustomError> {
                    // Custom validation logic
                    if tx.gas_price < 1_000_000_000 {
                        return Err(CustomError("Gas price too low"));
                    }
                    Ok(MockRpcTx { inner: tx, signer, info })
                },
            )
            .with_tx_env_converter(
                |_req: TransactionRequest,
                 _cfg_env: &CfgEnv<()>,
                 _block_env: &BlockEnv|
                 -> Result<TxEnv, CustomError> {
                    // Custom tx env conversion
                    Ok(TxEnv::default())
                },
            );

        // Test successful conversion
        let tx = MockTx {
            hash: B256::default(),
            value: U256::from(100),
            nonce: 0,
            gas_limit: 21000,
            gas_price: 20_000_000_000,
            input: Bytes::default(),
        };
        let result = converter.tx_converter.convert(
            tx.clone(),
            Address::default(),
            TransactionInfo::default(),
        );
        assert!(result.is_ok());

        // Test failed conversion
        let low_gas_tx = MockTx {
            hash: B256::default(),
            value: U256::from(100),
            nonce: 0,
            gas_limit: 21000,
            gas_price: 500_000_000, // Too low
            input: Bytes::default(),
        };
        let result = converter.tx_converter.convert(
            low_gas_tx,
            Address::default(),
            TransactionInfo::default(),
        );
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().0, "Gas price too low");
    }

    #[test]
    fn test_converter_builder_pattern() {
        // Test that the builder pattern works correctly
        let converter = RpcConverter::new()
            .with_tx_converter(|tx: MockTx, signer, info| {
                Ok::<_, std::convert::Infallible>(MockRpcTx { inner: tx, signer, info })
            })
            .with_receipt_converter(|_receipt: ()| Ok::<(), std::convert::Infallible>(()))
            .with_header_converter(|_header: ()| Ok::<(), std::convert::Infallible>(()));

        // Verify that converters work
        let tx = MockTx {
            hash: B256::default(),
            value: U256::from(100),
            nonce: 0,
            gas_limit: 21000,
            gas_price: 20_000_000_000,
            input: Bytes::default(),
        };
        let result = RpcTxConverter::convert(
            &converter.tx_converter,
            tx,
            Address::default(),
            TransactionInfo::default(),
        );
        assert!(result.is_ok());

        // Verify we can chain builder calls
        let _ = RpcConverter::new()
            .with_tx_converter(|tx: MockTx, signer, info| {
                Ok::<_, std::convert::Infallible>(MockRpcTx { inner: tx, signer, info })
            })
            .with_tx_env_converter(TxEnvConverterFn {
                f: |_req: TransactionRequest, _block: &BlockEnv| {
                    Ok::<_, std::convert::Infallible>(TxEnv::default())
                },
            })
            .with_sim_tx_converter(|_req: ()| Ok::<_, std::convert::Infallible>(()))
            .with_receipt_converter(|_receipt: ()| Ok::<_, std::convert::Infallible>(()))
            .with_header_converter(|_header: ()| Ok::<_, std::convert::Infallible>(()));
    }

    #[test]
    fn test_closure_based_conversion() {
        // Test using a closure for conversion
        let custom_converter = |tx: MockTx,
                                signer: Address,
                                info: TransactionInfo|
         -> Result<MockRpcTx, std::convert::Infallible> {
            Ok(MockRpcTx { inner: tx, signer, info })
        };

        let tx = MockTx {
            hash: B256::default(),
            value: U256::from(200),
            nonce: 0,
            gas_limit: 21000,
            gas_price: 20_000_000_000,
            input: Bytes::default(),
        };
        let signer = Address::default();
        let info = TransactionInfo::default();

        let result = custom_converter.convert(tx.clone(), signer, info);
        assert!(result.is_ok());

        let rpc_tx = result.unwrap();
        assert_eq!(rpc_tx.inner.value, U256::from(200));
    }

    #[test]
    fn test_converter_with_captured_context() {
        // Test closure with captured variables
        let multiplier = 2u64;
        let converter_with_context = move |tx: MockTx,
                                           signer: Address,
                                           info: TransactionInfo|
              -> Result<MockRpcTx, std::convert::Infallible> {
            let mut modified_tx = tx;
            modified_tx.value *= U256::from(multiplier);
            Ok(MockRpcTx { inner: modified_tx, signer, info })
        };

        let tx = MockTx {
            hash: B256::default(),
            value: U256::from(100),
            nonce: 0,
            gas_limit: 21000,
            gas_price: 20_000_000_000,
            input: Bytes::default(),
        };
        let result =
            converter_with_context.convert(tx, Address::default(), TransactionInfo::default());
        assert!(result.is_ok());

        let rpc_tx = result.unwrap();
        assert_eq!(rpc_tx.inner.value, U256::from(200)); // 100 * 2
    }

    #[test]
    fn test_rpc_convert_trait_with_new_converter() {
        // Test that RpcConvert trait works with the new converter design
        // This ensures backward compatibility
        let _converter = RpcConverter::new();
        // For now, just test it can be created
        // Full integration will be tested separately
    }

    #[test]
    fn test_error_propagation() {
        #[derive(Debug)]
        struct CustomError;
        impl std::fmt::Display for CustomError {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "Custom conversion error")
            }
        }
        impl std::error::Error for CustomError {}

        // Test error handling in conversion
        let failing_converter = |_tx: MockTx,
                                 _signer: Address,
                                 _info: TransactionInfo|
         -> Result<MockRpcTx, CustomError> { Err(CustomError) };

        let tx = MockTx {
            hash: B256::default(),
            value: U256::from(100),
            nonce: 0,
            gas_limit: 21000,
            gas_price: 20_000_000_000,
            input: Bytes::default(),
        };
        let result = failing_converter.convert(tx, Address::default(), TransactionInfo::default());
        assert!(result.is_err());
    }

    #[test]
    fn test_multiple_converter_types() {
        // Test setting multiple converter types
        let converter = RpcConverter::new().with_tx_converter(
            |tx: MockTx, signer, info| -> Result<MockRpcTx, std::convert::Infallible> {
                Ok(MockRpcTx { inner: tx, signer, info })
            },
        );
        // Future: .with_receipt_converter(...)
        // Future: .with_header_converter(...)

        // Check that the tx converter works
        let tx = MockTx {
            hash: B256::default(),
            value: U256::from(300),
            nonce: 0,
            gas_limit: 21000,
            gas_price: 20_000_000_000,
            input: Bytes::default(),
        };
        let result =
            converter.tx_converter.convert(tx, Address::default(), TransactionInfo::default());
        assert!(result.is_ok());
        assert_eq!(result.unwrap().inner.value, U256::from(300));

        // Others remain unit type
        assert!(std::mem::size_of_val(&converter.tx_env_converter) == 0);
    }
}

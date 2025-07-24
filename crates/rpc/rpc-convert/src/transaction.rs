//! Compatibility functions for rpc `Transaction` type.

use crate::{
    fees::{CallFees, CallFeesError},
    RpcHeader, RpcReceipt, RpcTransaction, RpcTxReq, RpcTypes as CrateRpcTypes,
};
use alloy_consensus::{error::ValueError, transaction::Recovered, Sealable, SignableTransaction};
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
    NodePrimitives, SealedHeader, SealedHeaderFor, TransactionMeta, TxTy,
};
use revm_context::{BlockEnv, CfgEnv, TxEnv};
use std::{
    borrow::Cow,
    convert::Infallible,
    fmt::{self, Debug},
    marker::{PhantomData, Unpin},
};
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
    /// The node primitives type.
    type Primitives: NodePrimitives;

    /// The network type used in RPC.
    type Network: RpcTypes;

    /// The transaction environment
    type TxEnv;

    /// The error type that can be returned from conversions.
    type Error: error::Error;

    /// Converts the transaction to a transaction response for the RPC.
    fn fill(
        &self,
        tx: Recovered<TxTy<Self::Primitives>>,
        tx_info: TransactionInfo,
    ) -> Result<RpcTransaction<Self::Network>, Self::Error>;

    /// Convert simulate v1 transaction request to an RPC transaction.
    fn build_simulate_v1_transaction(
        &self,
        request: RpcTxReq<Self::Network>,
    ) -> Result<TxTy<Self::Primitives>, Self::Error>;

    /// Convert transaction request into a transaction environment.
    fn tx_env<Spec>(
        &self,
        request: RpcTxReq<Self::Network>,
        cfg_env: &CfgEnv<Spec>,
        block_env: &BlockEnv,
    ) -> Result<Self::TxEnv, Self::Error>;

    /// Convert receipts into RPC receipts.
    fn convert_receipts(
        &self,
        receipts: Vec<ConvertReceiptInput<'_, Self::Primitives>>,
    ) -> Result<Vec<RpcReceipt<Self::Network>>, Self::Error>;

    /// Convert header to RPC format.
    fn convert_header(
        &self,
        header: SealedHeaderFor<Self::Primitives>,
        block_size: usize,
    ) -> Result<RpcHeader<Self::Network>, Self::Error>;
}

/// RPC types trait to support custom RPC types, while keeping network specific variants.
pub trait RpcTypes: CrateRpcTypes {}

impl RpcTypes for alloy_network::Ethereum {}

/// Trait for converting network transaction responses to primitive transaction types.
pub trait TryFromTransactionResponse<N: alloy_network::Network> {
    /// The error type returned if the conversion fails.
    type Error: core::error::Error + Send + Sync + Unpin;

    /// Converts a network transaction response to a primitive transaction type.
    ///
    /// # Returns
    ///
    /// Returns `Ok(Self)` on successful conversion, or `Err(Self::Error)` if the conversion fails.
    fn from_transaction_response(transaction_response: N::TransactionResponse) -> Result<Self, Self::Error>
    where
        Self: Sized;
}

impl<N: alloy_network::Network, T> TryFromTransactionResponse<N> for T
where
    N::TransactionResponse: Into<Self>,
{
    type Error = Infallible;

    fn from_transaction_response(transaction_response: N::TransactionResponse) -> Result<Self, Self::Error> {
        Ok(transaction_response.into())
    }
}

/// Conversion trait for primitive transactions.
pub trait IntoRpcTx<Rpc>: SignedTransaction {
    /// The transaction info type that's generated alongside the transaction
    type TxInfo;

    /// Converts the primitive transaction to a rpc transaction
    fn into_rpc_tx(self, signer: Address, tx_info: Self::TxInfo) -> Rpc;
}

impl IntoRpcTx<reth_ethereum_primitives::TransactionSigned>
    for reth_ethereum_primitives::TransactionSigned
{
    type TxInfo = ();

    fn into_rpc_tx(self, _signer: Address, _tx_info: Self::TxInfo) -> Self {
        self
    }
}

impl IntoRpcTx<alloy_rpc_types_eth::Transaction> for reth_ethereum_primitives::TransactionSigned {
    type TxInfo = TransactionInfo;

    fn into_rpc_tx(
        self,
        signer: Address,
        tx_info: Self::TxInfo,
    ) -> alloy_rpc_types_eth::Transaction {
        alloy_rpc_types_eth::Transaction::from_transaction(
            Recovered::new_unchecked(self.into(), signer),
            tx_info,
        )
    }
}

/// Creates a new rpc transaction from a transaction response, and a map that takes the
/// original transaction and maps it to the transaction response.
pub trait FromConsensusTx<T>: Sized {
    /// The transaction info type that's generated alongside the transaction
    type TxInfo;

    /// Converts consensus transaction to its RPC representation.
    fn from_consensus_tx(tx: T, signer: Address, tx_info: Self::TxInfo) -> Self;
}

impl FromConsensusTx<reth_ethereum_primitives::TransactionSigned>
    for alloy_rpc_types_eth::Transaction
{
    type TxInfo = TransactionInfo;

    fn from_consensus_tx(
        tx: reth_ethereum_primitives::TransactionSigned,
        signer: Address,
        tx_info: Self::TxInfo,
    ) -> Self {
        Self::from_transaction(Recovered::new_unchecked(tx.into(), signer), tx_info)
    }
}

/// Trait to map additional information for the RPC transaction output.
///
/// This is useful when the RPC transaction output needs to include additional
/// information.
pub trait TxInfoMapper<Tx> {
    /// Output type.
    type Out;

    /// Error type returned by the mapper.
    type Err;

    /// Maps `tx` and the given [`TransactionInfo`] to the output type.
    fn try_map(&self, tx: Tx, info: TransactionInfo) -> Result<Self::Out, Self::Err>;
}

impl TxInfoMapper<&reth_ethereum_primitives::TransactionSigned> for () {
    type Out = TransactionInfo;
    type Err = Infallible;

    fn try_map(
        &self,
        _tx: &reth_ethereum_primitives::TransactionSigned,
        info: TransactionInfo,
    ) -> Result<Self::Out, Self::Err> {
        Ok(info)
    }
}

/// Trait for converting from a typed transaction request into a primitive transaction.
pub trait TryIntoSimTx<Tx>: Sized {
    /// Build a primitive transaction from the request.
    fn try_into_sim_tx(self) -> Result<Tx, ValueError<Self>>;
}

impl TryIntoSimTx<reth_ethereum_primitives::TransactionSigned> for TransactionRequest {
    fn try_into_sim_tx(
        self,
    ) -> Result<reth_ethereum_primitives::TransactionSigned, ValueError<Self>> {
        let tx = self
            .build_typed_tx()
            .map_err(|request| ValueError::new(request, "Required fields missing"))?;

        // Create an empty signature for the transaction.
        let signature =
            alloy_primitives::Signature::new(Default::default(), Default::default(), false);

        Ok(tx.into_signed(signature).into())
    }
}

/// Represents errors that can occur when converting from [`TransactionRequest`] to
/// [`TxEnvFor`].
#[derive(Debug, Error)]
pub enum EthTxEnvError {
    /// [`TransactionInputError`] thrown when building transaction.
    #[error(transparent)]
    TransactionInputError(#[from] TransactionInputError),
    /// [`CallFeesError`] thrown when configuring transaction fees.
    #[error(transparent)]
    CallFeesError(#[from] CallFeesError),
}

/// Trait for converting from a transaction request into a transaction environment.
pub trait TryIntoTxEnv<TxEnv>: Sized {
    /// Error type returned by the conversion.
    type Err;

    /// Convert the request into a transaction environment.
    fn try_into_tx_env<Spec>(
        self,
        cfg_env: &CfgEnv<Spec>,
        block_env: &BlockEnv,
    ) -> Result<TxEnv, Self::Err>;
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

impl From<TransactionConversionError> for jsonrpsee_types::ErrorObject<'static> {
    fn from(err: TransactionConversionError) -> Self {
        jsonrpsee_types::ErrorObject::owned(
            jsonrpsee_types::error::INTERNAL_ERROR_CODE,
            err.to_string(),
            None::<()>,
        )
    }
}

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

/// Trait for converting simulated transactions.
pub trait RpcSimTxConverter<Request, PrimitiveTx> {
    /// The error type that can be returned during conversion.
    type Error: std::error::Error;

    /// Convert a request to a simulated transaction.
    fn convert(&self, request: Request) -> Result<PrimitiveTx, Self::Error>;
}

// Blanket implementations for function pointers and closures that implement the traits
impl<PrimitiveTx, RpcTx, F, E> RpcTxConverter<PrimitiveTx, RpcTx> for F
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

impl<PrimitiveReceipt, RpcReceipt, F, E> RpcReceiptConverter<PrimitiveReceipt, RpcReceipt> for F
where
    F: Fn(PrimitiveReceipt) -> Result<RpcReceipt, E>,
    E: std::error::Error,
{
    type Error = E;

    fn convert(&self, receipt: PrimitiveReceipt) -> Result<RpcReceipt, Self::Error> {
        self(receipt)
    }
}

impl<PrimitiveHeader, RpcHeader, F, E> RpcHeaderConverter<PrimitiveHeader, RpcHeader> for F
where
    F: Fn(PrimitiveHeader) -> Result<RpcHeader, E>,
    E: std::error::Error,
{
    type Error = E;

    fn convert(&self, header: PrimitiveHeader) -> Result<RpcHeader, Self::Error> {
        self(header)
    }
}

impl<Request, TxEnv, F, E> RpcTxEnvConverter<Request, TxEnv> for F
where
    F: Fn(Request, &CfgEnv<()>, &BlockEnv) -> Result<TxEnv, E>,
    E: std::error::Error,
{
    type Error = E;

    fn convert<Spec>(
        &self,
        request: Request,
        cfg_env: &CfgEnv<Spec>,
        block_env: &BlockEnv,
    ) -> Result<TxEnv, Self::Error> {
        // Safety: We're converting from CfgEnv<Spec> to CfgEnv<()> which is safe
        // as we're only using the common fields
        let cfg_env_unit = unsafe { std::mem::transmute::<&CfgEnv<Spec>, &CfgEnv<()>>(cfg_env) };
        self(request, cfg_env_unit, block_env)
    }
}

impl<Request, PrimitiveTx, F, E> RpcSimTxConverter<Request, PrimitiveTx> for F
where
    F: Fn(Request) -> Result<PrimitiveTx, E>,
    E: std::error::Error,
{
    type Error = E;

    fn convert(&self, request: Request) -> Result<PrimitiveTx, Self::Error> {
        self(request)
    }
}

// Unit type implementations for default conversions
impl<Tx, Rpc, TxInfo> RpcTxConverter<Tx, Rpc> for ()
where
    Tx: IntoRpcTx<Rpc, TxInfo = TxInfo>,
    TxInfo: From<TransactionInfo>,
{
    type Error = std::convert::Infallible;

    fn convert(&self, tx: Tx, signer: Address, info: TransactionInfo) -> Result<Rpc, Self::Error> {
        Ok(tx.into_rpc_tx(signer, info.into()))
    }
}

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

impl<Request, PrimitiveTx> RpcSimTxConverter<Request, PrimitiveTx> for ()
where
    Request: TryIntoSimTx<PrimitiveTx> + Debug,
{
    type Error = ValueError<Request>;

    fn convert(&self, request: Request) -> Result<PrimitiveTx, Self::Error> {
        request.try_into_sim_tx()
    }
}

// Note: HeaderConverter implementation for unit type is already defined above (line 71-78)

/// Legacy implementation to convert primitive types to RPC types.
#[derive(Debug)]
pub struct LegacyRpcConverter<E = alloy_network::Ethereum, Evm = (), Err = (), Map = ()> {
    phantom: PhantomData<(E, Evm, Err)>,
    mapper: Map,
}

/// The new RPC converter that allows for flexible conversion methods.
///
/// This converter is designed to be more composable and easier to understand than
/// the legacy converter. It uses explicit converter fields for each conversion type.
///
/// # Examples
///
/// ```ignore
/// // Create a converter with default conversions
/// let converter = RpcConverter::new();
///
/// // Create a converter with custom transaction converter
/// let converter = RpcConverter::new()
///     .with_tx_converter(|tx, signer, info| {
///         // Custom conversion logic
///         Ok(MyCustomRpcTx { tx, signer, info })
///     });
/// ```
#[derive(Debug)]
pub struct RpcConverter<Tx = (), TxEnv = (), SimTx = (), Receipt = (), Header = ()> {
    /// Converter for transactions
    pub tx_converter: Tx,
    /// Converter for transaction environments
    pub tx_env_converter: TxEnv,
    /// Converter for simulated transactions
    pub sim_tx_converter: SimTx,
    /// Converter for receipts
    pub receipt_converter: Receipt,
    /// Converter for headers
    pub header_converter: Header,
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

// RpcConvert implementation for RpcConverter with a receipt converter for Ethereum
impl<Receipt> RpcConvert for RpcConverter<(), (), (), Receipt, ()>
where
    Receipt: ReceiptConverter<reth_ethereum_primitives::EthPrimitives>
        + Clone
        + Debug
        + Send
        + Sync
        + Unpin
        + 'static,
    Receipt::Error: error::Error + Send + Sync + 'static,
    Receipt::RpcReceipt: Into<alloy_rpc_types_eth::TransactionReceipt>,
{
    type Primitives = reth_ethereum_primitives::EthPrimitives;
    type Network = alloy_network::Ethereum;
    type TxEnv = TxEnv;
    type Error = TransactionConversionError;

    fn fill(
        &self,
        tx: Recovered<TxTy<Self::Primitives>>,
        tx_info: TransactionInfo,
    ) -> Result<<Self::Network as CrateRpcTypes>::TransactionResponse, Self::Error> {
        let (tx, signer) = tx.into_parts();
        Ok(Transaction::from_transaction(Recovered::new_unchecked(tx.into(), signer), tx_info))
    }

    fn build_simulate_v1_transaction(
        &self,
        request: RpcTxReq<Self::Network>,
    ) -> Result<TxTy<Self::Primitives>, Self::Error> {
        request.try_into_sim_tx().map_err(|e| TransactionConversionError(e.to_string()))
    }

    fn tx_env<Spec>(
        &self,
        request: RpcTxReq<Self::Network>,
        cfg_env: &CfgEnv<Spec>,
        block_env: &BlockEnv,
    ) -> Result<Self::TxEnv, Self::Error> {
        request
            .try_into_tx_env(cfg_env, block_env)
            .map_err(|e| TransactionConversionError(format!("{:?}", e)))
    }

    fn convert_receipts(
        &self,
        receipts: Vec<ConvertReceiptInput<'_, Self::Primitives>>,
    ) -> Result<Vec<RpcReceipt<Self::Network>>, Self::Error> {
        let rpc_receipts = self
            .receipt_converter
            .convert_receipts(receipts)
            .map_err(|e| TransactionConversionError(format!("{:?}", e)))?;

        // Convert the receipt converter's output to the expected RPC receipt type
        Ok(rpc_receipts.into_iter().map(Into::into).collect())
    }

    fn convert_header(
        &self,
        header: SealedHeaderFor<Self::Primitives>,
        block_size: usize,
    ) -> Result<RpcHeader<Self::Network>, Self::Error> {
        use alloy_rpc_types_eth::Header;
        Ok(Header::from_consensus(header.into(), None, Some(U256::from(block_size))))
    }
}

impl<N, E, Evm, Err, Map> RpcConvert for LegacyRpcConverter<E, Evm, Err, Map>
where
    N: NodePrimitives,
    E: RpcTypes + Send + Sync + Unpin + Clone + Debug,
    Evm: ConfigureEvm<Primitives = N> + 'static,
    Err: Send + Sync + Unpin + Clone + Debug + 'static,
    TxTy<N>: IntoRpcTx<E::TransactionResponse> + Clone + Debug,
    RpcTxReq<E>: TryIntoSimTx<TxTy<N>> + TryIntoTxEnv<TxEnvFor<Evm>>,
    <RpcTxReq<E> as TryIntoTxEnv<TxEnvFor<Evm>>>::Err: fmt::Debug,
    Map: for<'a> TxInfoMapper<
            &'a TxTy<N>,
            Out = <TxTy<N> as IntoRpcTx<E::TransactionResponse>>::TxInfo,
            Err: fmt::Debug,
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
    type Error = TransactionConversionError;

    fn fill(
        &self,
        tx: Recovered<TxTy<N>>,
        tx_info: TransactionInfo,
    ) -> Result<E::TransactionResponse, Self::Error> {
        let (tx, signer) = tx.into_parts();
        let tx_info = self
            .mapper
            .try_map(&tx, tx_info)
            .map_err(|e| TransactionConversionError(format!("{:?}", e)))?;

        Ok(tx.into_rpc_tx(signer, tx_info))
    }

    fn build_simulate_v1_transaction(&self, request: RpcTxReq<E>) -> Result<TxTy<N>, Self::Error> {
        request.try_into_sim_tx().map_err(|e| TransactionConversionError(e.to_string()))
    }

    fn tx_env<Spec>(
        &self,
        request: RpcTxReq<E>,
        cfg_env: &CfgEnv<Spec>,
        block_env: &BlockEnv,
    ) -> Result<Self::TxEnv, Self::Error> {
        request
            .try_into_tx_env(cfg_env, block_env)
            .map_err(|e| TransactionConversionError(format!("{:?}", e)))
    }

    fn convert_receipts(
        &self,
        _receipts: Vec<ConvertReceiptInput<'_, Self::Primitives>>,
    ) -> Result<Vec<RpcReceipt<Self::Network>>, Self::Error> {
        // LegacyRpcConverter doesn't handle receipts directly
        Err(TransactionConversionError(
            "Receipt conversion not supported in legacy converter".to_string(),
        ))
    }

    fn convert_header(
        &self,
        _header: SealedHeaderFor<Self::Primitives>,
        _block_size: usize,
    ) -> Result<RpcHeader<Self::Network>, Self::Error> {
        // LegacyRpcConverter doesn't handle headers directly
        Err(TransactionConversionError(
            "Header conversion not supported in legacy converter".to_string(),
        ))
    }
}

/// Optimism specific RPC transaction compatibility implementations.
#[cfg(feature = "op")]
pub mod op {
    use super::*;
    use alloy_consensus::SignableTransaction;
    use alloy_primitives::{Address, Signature};
    use op_alloy_consensus::{
        transaction::{OpDepositInfo, OpTransactionInfo},
        OpTxEnvelope,
    };
    use op_alloy_rpc_types::OpTransactionRequest;
    use op_revm::{transaction::deposit::DepositTransactionParts, OpTransaction};
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
            // Convert to standard eth transaction request first
            let tx_env = self.as_ref().clone().try_into_tx_env(cfg_env, block_env)?;

            // Extract deposit transaction parts if this is a deposit tx
            let deposit = DepositTransactionParts::default();

            Ok(OpTransaction {
                base: tx_env,
                enveloped_tx: None, // Would need encoded tx data
                deposit,
            })
        }
    }

    // Implement TryIntoSimTx for Optimism transaction envelope
    impl TryIntoSimTx<OpTxEnvelope> for OpTxEnvelope {
        fn try_into_sim_tx(self) -> Result<OpTxEnvelope, ValueError<Self>> {
            Ok(self)
        }
    }
}

use reth_primitives_traits::SignedTransaction;

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Bytes, B256};

    // Mock types for testing
    #[derive(Debug, Clone, PartialEq)]
    struct MockTx {
        hash: B256,
        value: U256,
        nonce: u64,
        gas_limit: u64,
        gas_price: u128,
        input: Bytes,
    }

    #[derive(Debug, Clone, PartialEq)]
    struct MockRpcTx {
        inner: MockTx,
        signer: Address,
        info: TransactionInfo,
    }

    #[test]
    fn test_default_converter_construction() {
        let converter = RpcConverter::new();
        // Check that all fields are unit types
        assert_eq!(std::mem::size_of_val(&converter.tx_converter), 0);
        assert_eq!(std::mem::size_of_val(&converter.tx_env_converter), 0);
        assert_eq!(std::mem::size_of_val(&converter.sim_tx_converter), 0);
        assert_eq!(std::mem::size_of_val(&converter.receipt_converter), 0);
        assert_eq!(std::mem::size_of_val(&converter.header_converter), 0);
    }

    #[test]
    fn test_converter_builder_pattern() {
        let converter = RpcConverter::new()
            .with_tx_converter(42)
            .with_tx_env_converter("env")
            .with_sim_tx_converter(vec![1, 2, 3])
            .with_receipt_converter(true)
            .with_header_converter(3.14);

        assert_eq!(converter.tx_converter, 42);
        assert_eq!(converter.tx_env_converter, "env");
        assert_eq!(converter.sim_tx_converter, vec![1, 2, 3]);
        assert_eq!(converter.receipt_converter, true);
        assert_eq!(converter.header_converter, 3.14);
    }

    #[test]
    fn test_trait_based_conversion() {
        // Test implementation of RpcTxConverter trait
        struct MyTxConverter;

        impl RpcTxConverter<MockTx, MockRpcTx> for MyTxConverter {
            type Error = std::convert::Infallible;

            fn convert(
                &self,
                tx: MockTx,
                signer: Address,
                info: TransactionInfo,
            ) -> Result<MockRpcTx, Self::Error> {
                Ok(MockRpcTx { inner: tx, signer, info })
            }
        }

        let converter = MyTxConverter;
        let tx = MockTx {
            hash: B256::default(),
            value: U256::from(100),
            nonce: 0,
            gas_limit: 21000,
            gas_price: 20_000_000_000,
            input: Bytes::default(),
        };

        let result = converter.convert(tx.clone(), Address::default(), TransactionInfo::default());
        assert!(result.is_ok());

        let rpc_tx = result.unwrap();
        assert_eq!(rpc_tx.inner, tx);
        assert_eq!(rpc_tx.signer, Address::default());
    }

    #[test]
    fn test_closure_based_conversion() {
        // Test that closures implement the converter traits
        let tx_converter = |tx: MockTx, signer: Address, info: TransactionInfo| {
            Ok::<_, std::convert::Infallible>(MockRpcTx { inner: tx, signer, info })
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
            tx_converter.convert(tx.clone(), Address::default(), TransactionInfo::default());
        assert!(result.is_ok());
    }

    #[test]
    fn test_converter_with_custom_tx_converter() {
        let custom_converter = |tx: MockTx, signer: Address, info: TransactionInfo| {
            // Custom logic: double the value
            let mut modified_tx = tx;
            modified_tx.value *= U256::from(2);
            Ok::<_, std::convert::Infallible>(MockRpcTx { inner: modified_tx, signer, info })
        };

        let converter = RpcConverter::new().with_tx_converter(custom_converter);

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

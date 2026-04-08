//! Compatibility functions for rpc `Transaction` type.
use crate::{
    RpcHeader, RpcReceipt, RpcTransaction, RpcTxReq, RpcTypes, SignableTxRequest, TryIntoTxEnv,
};
use alloy_consensus::{error::ValueError, transaction::Recovered};
use alloy_primitives::Address;
use alloy_rpc_types_eth::TransactionInfo;
use core::error;
use dyn_clone::DynClone;
use reth_evm::{BlockEnvFor, ConfigureEvm, EvmEnvFor, SpecFor, TxEnvFor};
use reth_primitives_traits::{
    BlockTy, HeaderTy, NodePrimitives, SealedBlock, SealedHeader, SealedHeaderFor, TransactionMeta,
    TxTy,
};
use reth_rpc_traits::{FromConsensusHeader, FromConsensusTx, TryIntoSimTx, TxInfoMapper};
use std::{convert::Infallible, error::Error, fmt, fmt::Debug, marker::PhantomData};

/// Input for [`RpcConvert::convert_receipts`].
#[derive(Debug, Clone)]
pub struct ConvertReceiptInput<'a, N: NodePrimitives> {
    /// Primitive receipt.
    pub receipt: N::Receipt,
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

    /// Converts a set of primitive receipts to RPC representations. It is guaranteed that all
    /// receipts are from `block`.
    fn convert_receipts_with_block(
        &self,
        receipts: Vec<ConvertReceiptInput<'_, N>>,
        _block: &SealedBlock<N::Block>,
    ) -> Result<Vec<Self::RpcReceipt>, Self::Error> {
        self.convert_receipts(receipts)
    }
}

/// A type that knows how to convert a consensus header into an RPC header.
pub trait HeaderConverter<Consensus, Rpc>: Send + Sync + Unpin + Clone + 'static {
    /// An associated RPC conversion error.
    type Err: error::Error;

    /// Converts a consensus header into an RPC header.
    fn convert_header(
        &self,
        header: SealedHeader<Consensus>,
        block_size: usize,
    ) -> Result<Rpc, Self::Err>;
}

/// Default implementation of [`HeaderConverter`] that uses [`FromConsensusHeader`] to convert
/// headers.
impl<Consensus, Rpc> HeaderConverter<Consensus, Rpc> for ()
where
    Rpc: FromConsensusHeader<Consensus>,
{
    type Err = Infallible;

    fn convert_header(
        &self,
        header: SealedHeader<Consensus>,
        block_size: usize,
    ) -> Result<Rpc, Self::Err> {
        Ok(Rpc::from_consensus_header(header, block_size))
    }
}

impl<Consensus, Rpc, F> HeaderConverter<Consensus, Rpc> for F
where
    F: Fn(SealedHeader<Consensus>, usize) -> Rpc + Send + Sync + Unpin + Clone + 'static,
{
    type Err = Infallible;

    fn convert_header(
        &self,
        header: SealedHeader<Consensus>,
        block_size: usize,
    ) -> Result<Rpc, Self::Err> {
        Ok(self(header, block_size))
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
#[auto_impl::auto_impl(&, Box, Arc)]
pub trait RpcConvert: Send + Sync + Unpin + Debug + DynClone + 'static {
    /// Associated lower layer consensus types to convert from and into types of [`Self::Network`].
    type Primitives: NodePrimitives;

    /// The EVM configuration.
    type Evm: ConfigureEvm<Primitives = Self::Primitives>;

    /// Associated upper layer JSON-RPC API network requests and responses to convert from and into
    /// types of [`Self::Primitives`].
    type Network: RpcTypes<TransactionRequest: SignableTxRequest<TxTy<Self::Primitives>>>;

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
        tx_info: TransactionInfo,
    ) -> Result<RpcTransaction<Self::Network>, Self::Error>;

    /// Builds a fake transaction from a transaction request for inclusion into block built in
    /// `eth_simulateV1`.
    fn build_simulate_v1_transaction(
        &self,
        request: RpcTxReq<Self::Network>,
    ) -> Result<TxTy<Self::Primitives>, Self::Error>;

    /// Creates a transaction environment for execution based on `request` with corresponding
    /// `cfg_env` and `block_env`.
    fn tx_env(
        &self,
        request: RpcTxReq<Self::Network>,
        evm_env: &EvmEnvFor<Self::Evm>,
    ) -> Result<TxEnvFor<Self::Evm>, Self::Error>;

    /// Converts a set of primitive receipts to RPC representations. It is guaranteed that all
    /// receipts are from the same block.
    fn convert_receipts(
        &self,
        receipts: Vec<ConvertReceiptInput<'_, Self::Primitives>>,
    ) -> Result<Vec<RpcReceipt<Self::Network>>, Self::Error>;

    /// Converts a set of primitive receipts to RPC representations. It is guaranteed that all
    /// receipts are from the same block.
    ///
    /// Also accepts the corresponding block in case the receipt requires additional metadata.
    fn convert_receipts_with_block(
        &self,
        receipts: Vec<ConvertReceiptInput<'_, Self::Primitives>>,
        block: &SealedBlock<BlockTy<Self::Primitives>>,
    ) -> Result<Vec<RpcReceipt<Self::Network>>, Self::Error>;

    /// Converts a primitive header to an RPC header.
    fn convert_header(
        &self,
        header: SealedHeaderFor<Self::Primitives>,
        block_size: usize,
    ) -> Result<RpcHeader<Self::Network>, Self::Error>;
}

dyn_clone::clone_trait_object!(
    <Primitives, Network, Error, Evm>
    RpcConvert<Primitives = Primitives, Network = Network, Error = Error, Evm = Evm>
);

/// Converts `Tx` into `RpcTx`
///
/// Where:
/// * `Tx` is a transaction from the consensus layer.
/// * `RpcTx` is a transaction response object of the RPC API
///
/// The conversion function is accompanied by `signer`'s address and `tx_info` providing extra
/// context about a transaction in a block.
///
/// The `RpcTxConverter` has two blanket implementations:
/// * `()` assuming `RpcTx` implements [`FromConsensusTx`] and is used as default for
///   [`RpcConverter`].
/// * `Fn(Tx, Address, TxInfo) -> RpcTx` and can be applied using
///   [`RpcConverter::with_rpc_tx_converter`].
///
/// One should prefer to implement [`FromConsensusTx`] for `RpcTx` to get the `RpcTxConverter`
/// implementation for free, thanks to the blanket implementation, unless the conversion requires
/// more context. For example, some configuration parameters or access handles to database, network,
/// etc.
pub trait RpcTxConverter<Tx, RpcTx, TxInfo>: Clone + Unpin + Send + Sync + 'static {
    /// An associated error that can happen during the conversion.
    type Err;

    /// Performs the conversion of `tx` from `Tx` into `RpcTx`.
    ///
    /// See [`RpcTxConverter`] for more information.
    fn convert_rpc_tx(&self, tx: Tx, signer: Address, tx_info: TxInfo) -> Result<RpcTx, Self::Err>;
}

impl<Tx, RpcTx> RpcTxConverter<Tx, RpcTx, <RpcTx as FromConsensusTx<Tx>>::TxInfo> for ()
where
    RpcTx: FromConsensusTx<Tx>,
{
    type Err = RpcTx::Err;

    fn convert_rpc_tx(
        &self,
        tx: Tx,
        signer: Address,
        tx_info: <RpcTx as FromConsensusTx<Tx>>::TxInfo,
    ) -> Result<RpcTx, Self::Err> {
        RpcTx::from_consensus_tx(tx, signer, tx_info)
    }
}

impl<Tx, RpcTx, F, TxInfo, E> RpcTxConverter<Tx, RpcTx, TxInfo> for F
where
    F: Fn(Tx, Address, TxInfo) -> Result<RpcTx, E> + Clone + Unpin + Send + Sync + 'static,
{
    type Err = E;

    fn convert_rpc_tx(&self, tx: Tx, signer: Address, tx_info: TxInfo) -> Result<RpcTx, Self::Err> {
        self(tx, signer, tx_info)
    }
}

/// Converts `TxReq` into `SimTx`.
///
/// Where:
/// * `TxReq` is a transaction request received from an RPC API
/// * `SimTx` is the corresponding consensus layer transaction for execution simulation
///
/// The `SimTxConverter` has two blanket implementations:
/// * `()` assuming `TxReq` implements [`TryIntoSimTx`] and is used as default for [`RpcConverter`].
/// * `Fn(TxReq) -> Result<SimTx, ValueError<TxReq>>` and can be applied using
///   [`RpcConverter::with_sim_tx_converter`].
///
/// One should prefer to implement [`TryIntoSimTx`] for `TxReq` to get the `SimTxConverter`
/// implementation for free, thanks to the blanket implementation, unless the conversion requires
/// more context. For example, some configuration parameters or access handles to database, network,
/// etc.
pub trait SimTxConverter<TxReq, SimTx>: Clone + Unpin + Send + Sync + 'static {
    /// An associated error that can occur during the conversion.
    type Err: Error;

    /// Performs the conversion from `tx_req` into `SimTx`.
    ///
    /// See [`SimTxConverter`] for more information.
    fn convert_sim_tx(&self, tx_req: TxReq) -> Result<SimTx, Self::Err>;
}

impl<TxReq, SimTx> SimTxConverter<TxReq, SimTx> for ()
where
    TxReq: TryIntoSimTx<SimTx> + Debug,
{
    type Err = ValueError<TxReq>;

    fn convert_sim_tx(&self, tx_req: TxReq) -> Result<SimTx, Self::Err> {
        tx_req.try_into_sim_tx()
    }
}

impl<TxReq, SimTx, F, E> SimTxConverter<TxReq, SimTx> for F
where
    TxReq: Debug,
    E: Error,
    F: Fn(TxReq) -> Result<SimTx, E> + Clone + Unpin + Send + Sync + 'static,
{
    type Err = E;

    fn convert_sim_tx(&self, tx_req: TxReq) -> Result<SimTx, Self::Err> {
        self(tx_req)
    }
}

/// Converts `TxReq` into `TxEnv`.
///
/// Where:
/// * `TxReq` is a transaction request received from an RPC API
/// * `TxEnv` is the corresponding transaction environment for execution
///
/// The `TxEnvConverter` has two blanket implementations:
/// * `()` assuming `TxReq` implements [`TryIntoTxEnv`] and is used as default for [`RpcConverter`].
/// * `Fn(TxReq, &CfgEnv<Spec>, &BlockEnv) -> Result<TxEnv, E>` and can be applied using
///   [`RpcConverter::with_tx_env_converter`].
///
/// One should prefer to implement [`TryIntoTxEnv`] for `TxReq` to get the `TxEnvConverter`
/// implementation for free, thanks to the blanket implementation, unless the conversion requires
/// more context. For example, some configuration parameters or access handles to database, network,
/// etc.
pub trait TxEnvConverter<TxReq, Evm: ConfigureEvm>:
    Debug + Send + Sync + Unpin + Clone + 'static
{
    /// An associated error that can occur during conversion.
    type Error;

    /// Converts a rpc transaction request into a transaction environment.
    ///
    /// See [`TxEnvConverter`] for more information.
    fn convert_tx_env(
        &self,
        tx_req: TxReq,
        evm_env: &EvmEnvFor<Evm>,
    ) -> Result<TxEnvFor<Evm>, Self::Error>;
}

impl<TxReq, Evm> TxEnvConverter<TxReq, Evm> for ()
where
    TxReq: TryIntoTxEnv<TxEnvFor<Evm>, SpecFor<Evm>, BlockEnvFor<Evm>>,
    Evm: ConfigureEvm,
{
    type Error = TxReq::Err;

    fn convert_tx_env(
        &self,
        tx_req: TxReq,
        evm_env: &EvmEnvFor<Evm>,
    ) -> Result<TxEnvFor<Evm>, Self::Error> {
        tx_req.try_into_tx_env(evm_env)
    }
}

/// Converts rpc transaction requests into transaction environment using a closure.
impl<F, TxReq, E, Evm> TxEnvConverter<TxReq, Evm> for F
where
    F: Fn(TxReq, &EvmEnvFor<Evm>) -> Result<TxEnvFor<Evm>, E>
        + Debug
        + Send
        + Sync
        + Unpin
        + Clone
        + 'static,
    TxReq: Clone,
    Evm: ConfigureEvm,
    E: error::Error + Send + Sync + 'static,
{
    type Error = E;

    fn convert_tx_env(
        &self,
        tx_req: TxReq,
        evm_env: &EvmEnvFor<Evm>,
    ) -> Result<TxEnvFor<Evm>, Self::Error> {
        self(tx_req, evm_env)
    }
}

/// Conversion into transaction RPC response failed.
#[derive(Debug, thiserror::Error)]
pub enum TransactionConversionError {
    /// Required fields are missing from the transaction request.
    #[error("Failed to convert transaction into RPC response: {0}")]
    FromTxReq(String),

    /// Other conversion errors.
    #[error("{0}")]
    Other(String),
}
/// Generic RPC response object converter for `Evm` and network `Network`.
///
/// The main purpose of this struct is to provide an implementation of [`RpcConvert`] for generic
/// associated types. This struct can then be used for conversions in RPC method handlers.
///
/// An [`RpcConvert`] implementation is generated if the following traits are implemented for the
/// network and EVM associated primitives:
/// * [`FromConsensusTx`]: from signed transaction into RPC response object.
/// * [`TryIntoSimTx`]: from RPC transaction request into a simulated transaction.
/// * [`TryIntoTxEnv`] or [`TxEnvConverter`]: from RPC transaction request into an executable
///   transaction.
/// * [`TxInfoMapper`]: from [`TransactionInfo`] into [`FromConsensusTx::TxInfo`]. Should be
///   implemented for a dedicated struct that is assigned to `Map`. If [`FromConsensusTx::TxInfo`]
///   is [`TransactionInfo`] then `()` can be used as `Map` which trivially passes over the input
///   object.
pub struct RpcConverter<
    Network,
    Evm,
    Receipt,
    Header = (),
    Map = (),
    SimTx = (),
    RpcTx = (),
    TxEnv = (),
> {
    network: PhantomData<Network>,
    evm: PhantomData<Evm>,
    receipt_converter: Receipt,
    header_converter: Header,
    mapper: Map,
    tx_env_converter: TxEnv,
    sim_tx_converter: SimTx,
    rpc_tx_converter: RpcTx,
}

impl<Network, Evm, Receipt, Header, Map, SimTx, RpcTx, TxEnv> fmt::Debug
    for RpcConverter<Network, Evm, Receipt, Header, Map, SimTx, RpcTx, TxEnv>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RpcConverter").finish_non_exhaustive()
    }
}

impl<Network, Evm, Receipt> RpcConverter<Network, Evm, Receipt> {
    /// Creates a new [`RpcConverter`] with `receipt_converter` and `mapper`.
    pub const fn new(receipt_converter: Receipt) -> Self {
        Self {
            network: PhantomData,
            evm: PhantomData,
            receipt_converter,
            header_converter: (),
            mapper: (),
            tx_env_converter: (),
            sim_tx_converter: (),
            rpc_tx_converter: (),
        }
    }
}

impl<Network, Evm, Receipt, Header, Map, SimTx, RpcTx, TxEnv>
    RpcConverter<Network, Evm, Receipt, Header, Map, SimTx, RpcTx, TxEnv>
{
    /// Converts the network type
    pub fn with_network<N>(
        self,
    ) -> RpcConverter<N, Evm, Receipt, Header, Map, SimTx, RpcTx, TxEnv> {
        let Self {
            receipt_converter,
            header_converter,
            mapper,
            evm,
            sim_tx_converter,
            rpc_tx_converter,
            tx_env_converter,
            ..
        } = self;
        RpcConverter {
            receipt_converter,
            header_converter,
            mapper,
            network: Default::default(),
            evm,
            sim_tx_converter,
            rpc_tx_converter,
            tx_env_converter,
        }
    }

    /// Converts the transaction environment type.
    pub fn with_tx_env_converter<TxEnvNew>(
        self,
        tx_env_converter: TxEnvNew,
    ) -> RpcConverter<Network, Evm, Receipt, Header, Map, SimTx, RpcTx, TxEnvNew> {
        let Self {
            receipt_converter,
            header_converter,
            mapper,
            network,
            evm,
            sim_tx_converter,
            rpc_tx_converter,
            tx_env_converter: _,
            ..
        } = self;
        RpcConverter {
            receipt_converter,
            header_converter,
            mapper,
            network,
            evm,
            sim_tx_converter,
            rpc_tx_converter,
            tx_env_converter,
        }
    }

    /// Configures the header converter.
    pub fn with_header_converter<HeaderNew>(
        self,
        header_converter: HeaderNew,
    ) -> RpcConverter<Network, Evm, Receipt, HeaderNew, Map, SimTx, RpcTx, TxEnv> {
        let Self {
            receipt_converter,
            header_converter: _,
            mapper,
            network,
            evm,
            sim_tx_converter,
            rpc_tx_converter,
            tx_env_converter,
        } = self;
        RpcConverter {
            receipt_converter,
            header_converter,
            mapper,
            network,
            evm,
            sim_tx_converter,
            rpc_tx_converter,
            tx_env_converter,
        }
    }

    /// Configures the mapper.
    pub fn with_mapper<MapNew>(
        self,
        mapper: MapNew,
    ) -> RpcConverter<Network, Evm, Receipt, Header, MapNew, SimTx, RpcTx, TxEnv> {
        let Self {
            receipt_converter,
            header_converter,
            mapper: _,
            network,
            evm,
            sim_tx_converter,
            rpc_tx_converter,
            tx_env_converter,
        } = self;
        RpcConverter {
            receipt_converter,
            header_converter,
            mapper,
            network,
            evm,
            sim_tx_converter,
            rpc_tx_converter,
            tx_env_converter,
        }
    }

    /// Swaps the simulate transaction converter with `sim_tx_converter`.
    pub fn with_sim_tx_converter<SimTxNew>(
        self,
        sim_tx_converter: SimTxNew,
    ) -> RpcConverter<Network, Evm, Receipt, Header, Map, SimTxNew, RpcTx, TxEnv> {
        let Self {
            receipt_converter,
            header_converter,
            mapper,
            network,
            evm,
            rpc_tx_converter,
            tx_env_converter,
            ..
        } = self;
        RpcConverter {
            receipt_converter,
            header_converter,
            mapper,
            network,
            evm,
            sim_tx_converter,
            rpc_tx_converter,
            tx_env_converter,
        }
    }

    /// Swaps the RPC transaction converter with `rpc_tx_converter`.
    pub fn with_rpc_tx_converter<RpcTxNew>(
        self,
        rpc_tx_converter: RpcTxNew,
    ) -> RpcConverter<Network, Evm, Receipt, Header, Map, SimTx, RpcTxNew, TxEnv> {
        let Self {
            receipt_converter,
            header_converter,
            mapper,
            network,
            evm,
            sim_tx_converter,
            tx_env_converter,
            ..
        } = self;
        RpcConverter {
            receipt_converter,
            header_converter,
            mapper,
            network,
            evm,
            sim_tx_converter,
            rpc_tx_converter,
            tx_env_converter,
        }
    }

    /// Converts `self` into a boxed converter.
    pub fn erased(
        self,
    ) -> Box<
        dyn RpcConvert<
            Primitives = <Self as RpcConvert>::Primitives,
            Network = <Self as RpcConvert>::Network,
            Error = <Self as RpcConvert>::Error,
            Evm = <Self as RpcConvert>::Evm,
        >,
    >
    where
        Self: RpcConvert,
    {
        Box::new(self)
    }
}

impl<Network, Evm, Receipt, Header, Map, SimTx, RpcTx, TxEnv> Default
    for RpcConverter<Network, Evm, Receipt, Header, Map, SimTx, RpcTx, TxEnv>
where
    Receipt: Default,
    Header: Default,
    Map: Default,
    SimTx: Default,
    RpcTx: Default,
    TxEnv: Default,
{
    fn default() -> Self {
        Self {
            network: Default::default(),
            evm: Default::default(),
            receipt_converter: Default::default(),
            header_converter: Default::default(),
            mapper: Default::default(),
            sim_tx_converter: Default::default(),
            rpc_tx_converter: Default::default(),
            tx_env_converter: Default::default(),
        }
    }
}

impl<
        Network,
        Evm,
        Receipt: Clone,
        Header: Clone,
        Map: Clone,
        SimTx: Clone,
        RpcTx: Clone,
        TxEnv: Clone,
    > Clone for RpcConverter<Network, Evm, Receipt, Header, Map, SimTx, RpcTx, TxEnv>
{
    fn clone(&self) -> Self {
        Self {
            network: Default::default(),
            evm: Default::default(),
            receipt_converter: self.receipt_converter.clone(),
            header_converter: self.header_converter.clone(),
            mapper: self.mapper.clone(),
            sim_tx_converter: self.sim_tx_converter.clone(),
            rpc_tx_converter: self.rpc_tx_converter.clone(),
            tx_env_converter: self.tx_env_converter.clone(),
        }
    }
}

impl<N, Network, Evm, Receipt, Header, Map, SimTx, RpcTx, TxEnv> RpcConvert
    for RpcConverter<Network, Evm, Receipt, Header, Map, SimTx, RpcTx, TxEnv>
where
    N: NodePrimitives,
    Network: RpcTypes<TransactionRequest: SignableTxRequest<N::SignedTx>>,
    Evm: ConfigureEvm<Primitives = N> + 'static,
    Receipt: ReceiptConverter<
            N,
            RpcReceipt = RpcReceipt<Network>,
            Error: From<TransactionConversionError>
                       + From<TxEnv::Error>
                       + From<<Map as TxInfoMapper<TxTy<N>>>::Err>
                       + From<RpcTx::Err>
                       + From<Header::Err>
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
    Header: HeaderConverter<HeaderTy<N>, RpcHeader<Network>>,
    Map: TxInfoMapper<TxTy<N>> + Clone + Debug + Unpin + Send + Sync + 'static,
    SimTx: SimTxConverter<RpcTxReq<Network>, TxTy<N>>,
    RpcTx:
        RpcTxConverter<TxTy<N>, Network::TransactionResponse, <Map as TxInfoMapper<TxTy<N>>>::Out>,
    TxEnv: TxEnvConverter<RpcTxReq<Network>, Evm>,
{
    type Primitives = N;
    type Evm = Evm;
    type Network = Network;
    type Error = Receipt::Error;

    fn fill(
        &self,
        tx: Recovered<TxTy<N>>,
        tx_info: TransactionInfo,
    ) -> Result<Network::TransactionResponse, Self::Error> {
        let (tx, signer) = tx.into_parts();
        let tx_info = self.mapper.try_map(&tx, tx_info)?;

        self.rpc_tx_converter.convert_rpc_tx(tx, signer, tx_info).map_err(Into::into)
    }

    fn build_simulate_v1_transaction(
        &self,
        request: RpcTxReq<Network>,
    ) -> Result<TxTy<N>, Self::Error> {
        Ok(self
            .sim_tx_converter
            .convert_sim_tx(request)
            .map_err(|e| TransactionConversionError::FromTxReq(e.to_string()))?)
    }

    fn tx_env(
        &self,
        request: RpcTxReq<Network>,
        evm_env: &EvmEnvFor<Evm>,
    ) -> Result<TxEnvFor<Evm>, Self::Error> {
        self.tx_env_converter.convert_tx_env(request, evm_env).map_err(Into::into)
    }

    fn convert_receipts(
        &self,
        receipts: Vec<ConvertReceiptInput<'_, Self::Primitives>>,
    ) -> Result<Vec<RpcReceipt<Self::Network>>, Self::Error> {
        self.receipt_converter.convert_receipts(receipts)
    }

    fn convert_receipts_with_block(
        &self,
        receipts: Vec<ConvertReceiptInput<'_, Self::Primitives>>,
        block: &SealedBlock<BlockTy<Self::Primitives>>,
    ) -> Result<Vec<RpcReceipt<Self::Network>>, Self::Error> {
        self.receipt_converter.convert_receipts_with_block(receipts, block)
    }

    fn convert_header(
        &self,
        header: SealedHeaderFor<Self::Primitives>,
        block_size: usize,
    ) -> Result<RpcHeader<Self::Network>, Self::Error> {
        Ok(self.header_converter.convert_header(header, block_size)?)
    }
}

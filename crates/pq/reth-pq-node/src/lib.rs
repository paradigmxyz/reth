//! # reth-pq-node
//!
//! Post-quantum Ethereum node definition.
//!
//! Provides [`PqNode`] — a [`NodeTypes`] implementation that wires
//! post-quantum components (ML-DSA-65 signatures, PQ precompile) into
//! the reth node builder pipeline.
//!
//! ## Architecture
//!
//! The PQ node reuses most Ethereum infrastructure but replaces:
//! - Transaction primitives (`PqSignedTransaction` instead of `TransactionSigned`)
//! - EVM configuration (`PqEvmConfig` with ML-DSA precompile)
//! - Receipt builder (`PqReceiptBuilder` for PQ transaction types)
//! - Transaction pool (`PqPoolBuilder` with `PqPoolValidator`)
//! - Engine types (`PqEngineTypes` with PQ-aware payload conversions)
//!
//! ## Status
//!
//! - `NodeTypes` impl: **complete**
//! - `PqPoolBuilder`: **complete**
//! - `PqExecutorBuilder`: **complete** (in `reth-pq-evm`)
//! - `PqConsensusBuilder`: **complete**
//! - `PqNetworkBuilder`: **complete**
//! - `PqEngineTypes`: **complete**
//! - `PqBuiltPayload` + conversions: **complete**
//! - `PqPayloadBuilder`: **complete**
//! - `Node<N>` wiring: **complete**

extern crate alloc;

pub mod engine;
pub mod payload;
pub mod rpc;

use alloc::sync::Arc;

use alloy_eips::eip7685::Requests;
use alloy_primitives::U256;
use alloy_rpc_types_engine::{
    BlobsBundleV1, BlobsBundleV2, ExecutionPayload, ExecutionPayloadEnvelopeV2,
    ExecutionPayloadEnvelopeV3, ExecutionPayloadEnvelopeV4, ExecutionPayloadEnvelopeV5,
    ExecutionPayloadEnvelopeV6, ExecutionPayloadFieldV2, ExecutionPayloadV1,
    ExecutionPayloadV3,
};
use reth_chainspec::{ChainSpec, EthChainSpec, EthereumHardforks};
use reth_engine_local::LocalPayloadAttributesBuilder;
use reth_engine_primitives::EngineTypes;
use reth_ethereum_consensus::EthBeaconConsensus;
use reth_ethereum_engine_primitives::{
    EthBuiltPayload, EthPayloadAttributes, EthPayloadBuilderAttributes,
};
use reth_evm::ConfigureEvm;
use reth_network::{primitives::BasicNetworkPrimitives, NetworkHandle, PeersInfo};
use reth_node_api::{FullNodeTypes, NodePrimitives, PrimitivesTy, TxTy};
use reth_node_builder::{
    components::{
        BasicPayloadServiceBuilder, ComponentsBuilder, ConsensusBuilder, NetworkBuilder,
        PoolBuilder,
    },
    DebugNode,
    node::{Node, NodeTypes},
    rpc::{
        BasicEngineApiBuilder, BasicEngineValidatorBuilder, PayloadValidatorBuilder, RpcAddOns,
    },
    AddOnsContext, BuilderContext, FullNodeComponents, NodeAdapter,
};
use reth_payload_primitives::{BuiltPayload, PayloadTypes};
use reth_pq_node_primitives::PqPrimitives;
use reth_pq_pool::{PqPoolValidator, PqPooledTransaction};
use reth_primitives_traits::SealedBlock;
use reth_provider::EthStorage;
use reth_transaction_pool::{
    blobstore::DiskFileBlobStore, CoinbaseTipOrdering, PoolPooledTx, PoolTransaction,
    TransactionPool,
};
use tracing::info;

// Re-export key types for downstream use
pub use reth_pq_evm::{PqEvmConfig, PqEvmFactory, PqExecutorBuilder, PqReceiptBuilder};
pub use rpc::{PqEthApiBuilder, PqRpcTxConverter};

// ─── PqAddOns ────────────────────────────────────────────────────────────────

/// Add-ons for the PQ node.
///
/// Wraps [`RpcAddOns`] with PQ-specific builders:
/// - [`PqEthApiBuilder`] — ETH API with PQ transaction conversion
/// - [`PqEngineValidatorBuilder`] — engine validator for PQ payloads
/// - [`BasicEngineApiBuilder`] — standard engine API builder
/// - [`BasicEngineValidatorBuilder`] — standard engine validator builder
///
/// The `N` parameter is a fully wired [`NodeAdapter`] — it can't be `PqNode`
/// directly because `RpcAddOns` requires `N: FullNodeComponents`.
pub type PqAddOns<N> = RpcAddOns<
    N,
    PqEthApiBuilder,
    PqEngineValidatorBuilder,
    BasicEngineApiBuilder<PqEngineValidatorBuilder>,
    BasicEngineValidatorBuilder<PqEngineValidatorBuilder>,
>;

// ─── PqBuiltPayload ──────────────────────────────────────────────────────────

/// Newtype wrapper around [`EthBuiltPayload<PqPrimitives>`].
///
/// Required because the `TryFrom`/`From` conversions to `ExecutionPayloadEnvelope*`
/// are only implemented for `EthBuiltPayload` (default `EthPrimitives`). Due to
/// Rust's orphan rules, we can't add `TryFrom<EthBuiltPayload<PqPrimitives>>`
/// for foreign types — so we wrap in a local type.
///
/// PQ transactions have no blob sidecars, so blob bundles are always empty.
#[derive(Debug, Clone)]
pub struct PqBuiltPayload(pub EthBuiltPayload<PqPrimitives>);

impl PqBuiltPayload {
    /// Creates a new PQ built payload.
    pub const fn new(inner: EthBuiltPayload<PqPrimitives>) -> Self {
        Self(inner)
    }

    /// Returns a reference to the inner `EthBuiltPayload`.
    pub const fn inner(&self) -> &EthBuiltPayload<PqPrimitives> {
        &self.0
    }
}

impl BuiltPayload for PqBuiltPayload {
    type Primitives = PqPrimitives;

    fn block(&self) -> &SealedBlock<<PqPrimitives as NodePrimitives>::Block> {
        self.0.block()
    }

    fn fees(&self) -> U256 {
        self.0.fees()
    }

    fn requests(&self) -> Option<Requests> {
        self.0.requests()
    }
}

// ─── Payload envelope conversions ────────────────────────────────────────────
//
// All `from_block_unchecked` functions require `T: Encodable2718` (+ `Transaction`
// for the aggregate `ExecutionPayload`). PqSignedTransaction implements both.
// PQ transactions have no blobs, so blob bundles are always empty.

/// V1: `engine_getPayloadV1` response
impl From<PqBuiltPayload> for ExecutionPayloadV1 {
    fn from(value: PqBuiltPayload) -> Self {
        let block = value.0.block().clone();
        Self::from_block_unchecked(block.hash(), &block.into_block())
    }
}

/// V2: `engine_getPayloadV2` response
impl From<PqBuiltPayload> for ExecutionPayloadEnvelopeV2 {
    fn from(value: PqBuiltPayload) -> Self {
        let block = value.0.block().clone();
        let fees = value.0.fees();
        Self {
            block_value: fees,
            execution_payload: ExecutionPayloadFieldV2::from_block_unchecked(
                block.hash(),
                &block.into_block(),
            ),
        }
    }
}

/// V3: `engine_getPayloadV3` response (Cancun)
impl From<PqBuiltPayload> for ExecutionPayloadEnvelopeV3 {
    fn from(value: PqBuiltPayload) -> Self {
        let block = value.0.block().clone();
        let fees = value.0.fees();
        // PQ transactions never carry blobs
        Self {
            execution_payload: ExecutionPayloadV3::from_block_unchecked(
                block.hash(),
                &block.into_block(),
            ),
            block_value: fees,
            should_override_builder: false,
            blobs_bundle: BlobsBundleV1::empty(),
        }
    }
}

/// V4: `engine_getPayloadV4` response (Prague)
impl From<PqBuiltPayload> for ExecutionPayloadEnvelopeV4 {
    fn from(value: PqBuiltPayload) -> Self {
        let requests = value.0.requests().unwrap_or_default();
        let v3: ExecutionPayloadEnvelopeV3 = value.into();
        Self {
            execution_requests: requests,
            envelope_inner: v3,
        }
    }
}

/// V5: `engine_getPayloadV5` response (Osaka)
impl From<PqBuiltPayload> for ExecutionPayloadEnvelopeV5 {
    fn from(value: PqBuiltPayload) -> Self {
        let block = value.0.block().clone();
        let fees = value.0.fees();
        let requests = value.0.requests().unwrap_or_default();
        Self {
            execution_payload: ExecutionPayloadV3::from_block_unchecked(
                block.hash(),
                &block.into_block(),
            ),
            block_value: fees,
            should_override_builder: false,
            blobs_bundle: BlobsBundleV2::empty(),
            execution_requests: requests,
        }
    }
}

/// V6: `engine_getPayloadV6` response (Amsterdam — not yet specified)
impl From<PqBuiltPayload> for ExecutionPayloadEnvelopeV6 {
    fn from(_value: PqBuiltPayload) -> Self {
        unimplemented!("ExecutionPayloadEnvelopeV6 not yet supported")
    }
}

// ─── PqPayloadTypes ──────────────────────────────────────────────────────────

/// Payload types for the PQ node.
///
/// Uses [`PqBuiltPayload`] as the built payload type. Reuses Ethereum payload
/// attributes (which are signature-agnostic).
#[derive(Debug, Default, Clone, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
pub struct PqPayloadTypes;

impl PayloadTypes for PqPayloadTypes {
    type BuiltPayload = PqBuiltPayload;
    type PayloadAttributes = EthPayloadAttributes;
    type PayloadBuilderAttributes = EthPayloadBuilderAttributes;
    type ExecutionData = alloy_rpc_types_engine::ExecutionData;

    fn block_to_payload(
        block: SealedBlock<
            <<Self::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
    ) -> Self::ExecutionData {
        let hash = block.hash();
        let block = block.into_block();
        let (payload, sidecar) =
            ExecutionPayload::from_block_unchecked(hash, &block);
        alloy_rpc_types_engine::ExecutionData { payload, sidecar }
    }
}

// ─── PqEngineTypes ───────────────────────────────────────────────────────────

/// Engine types for the PQ node.
///
/// Implements [`EngineTypes`] using the standard Ethereum execution payload
/// envelope types. This works because `from_block_unchecked` only requires
/// `T: Encodable2718` (+ `Transaction` for the aggregate variant), and
/// `PqSignedTransaction` implements both.
#[derive(Debug, Default, Clone, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
pub struct PqEngineTypes;

impl PayloadTypes for PqEngineTypes {
    type BuiltPayload = PqBuiltPayload;
    type PayloadAttributes = EthPayloadAttributes;
    type PayloadBuilderAttributes = EthPayloadBuilderAttributes;
    type ExecutionData = alloy_rpc_types_engine::ExecutionData;

    fn block_to_payload(
        block: SealedBlock<
            <<Self::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
    ) -> Self::ExecutionData {
        PqPayloadTypes::block_to_payload(block)
    }
}

impl EngineTypes for PqEngineTypes {
    type ExecutionPayloadEnvelopeV1 = ExecutionPayloadV1;
    type ExecutionPayloadEnvelopeV2 = ExecutionPayloadEnvelopeV2;
    type ExecutionPayloadEnvelopeV3 = ExecutionPayloadEnvelopeV3;
    type ExecutionPayloadEnvelopeV4 = ExecutionPayloadEnvelopeV4;
    type ExecutionPayloadEnvelopeV5 = ExecutionPayloadEnvelopeV5;
    type ExecutionPayloadEnvelopeV6 = ExecutionPayloadEnvelopeV6;
}

// ─── PqNode ──────────────────────────────────────────────────────────────────

/// Post-quantum Ethereum node type.
///
/// Implements [`NodeTypes`] with `Primitives = PqPrimitives` and
/// `Payload = PqEngineTypes`.
///
/// Also implements [`Node<N>`] which provides the full component builder
/// and add-ons for launching a PQ node.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct PqNode;

impl NodeTypes for PqNode {
    type Primitives = PqPrimitives;
    type ChainSpec = ChainSpec;
    type Storage = EthStorage<reth_pq_primitives::PqSignedTransaction>;
    type Payload = PqEngineTypes;
}

impl<N> Node<N> for PqNode
where
    N: FullNodeTypes<Types = Self>,
{
    type ComponentsBuilder = ComponentsBuilder<
        N,
        PqPoolBuilder,
        BasicPayloadServiceBuilder<PqPayloadBuilderComponent>,
        PqNetworkBuilder,
        PqExecutorBuilder,
        PqConsensusBuilder,
    >;

    type AddOns = PqAddOns<NodeAdapter<N>>;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        ComponentsBuilder::default()
            .node_types::<N>()
            .executor(PqExecutorBuilder::default())
            .consensus(PqConsensusBuilder)
            .pool(PqPoolBuilder)
            .payload(BasicPayloadServiceBuilder::new(PqPayloadBuilderComponent))
            .network(PqNetworkBuilder)
    }

    fn add_ons(&self) -> Self::AddOns {
        RpcAddOns::new(
            PqEthApiBuilder,
            PqEngineValidatorBuilder::default(),
            BasicEngineApiBuilder::default(),
            BasicEngineValidatorBuilder::default(),
            Default::default(), // Identity middleware
        )
    }
}

// ─── DebugNode (enables --dev mode auto-mining) ──────────────────────────────

impl<N: FullNodeComponents<Types = Self>> DebugNode<N> for PqNode {
    type RpcBlock = alloy_rpc_types_eth::Block;

    fn rpc_to_primitive_block(
        _rpc_block: alloy_rpc_types_eth::Block,
    ) -> reth_pq_node_primitives::Block {
        // PQ node doesn't support debug RPC consensus client (--debug.rpc-consensus-ws).
        // RPC blocks contain standard Ethereum transactions which can't be converted to PQ txs.
        // This path is only triggered by --debug.rpc-consensus-ws, not by --dev mode.
        unimplemented!(
            "PQ node does not support --debug.rpc-consensus-ws (cannot convert ECDSA txs to PQ)"
        )
    }

    fn local_payload_attributes_builder(
        chain_spec: &<Self as NodeTypes>::ChainSpec,
    ) -> impl reth_payload_primitives::PayloadAttributesBuilder<
        <PqEngineTypes as PayloadTypes>::PayloadAttributes,
        alloy_consensus::Header,
    > {
        LocalPayloadAttributesBuilder::new(Arc::new(chain_spec.clone()))
    }
}

// ─── PqPoolBuilder ───────────────────────────────────────────────────────────

/// Transaction pool type for the PQ node.
///
/// The `Client` parameter is the state provider factory (typically the node's
/// blockchain database provider).
pub type PqTransactionPool<Client> = reth_transaction_pool::Pool<
    PqPoolValidator<Client>,
    CoinbaseTipOrdering<PqPooledTransaction>,
    DiskFileBlobStore,
>;

/// Pool builder for the PQ node.
///
/// Creates a transaction pool using [`PqPoolValidator`] for ML-DSA-65
/// signature verification with full state access (nonce + balance checks).
/// The pool uses [`CoinbaseTipOrdering`] and [`DiskFileBlobStore`].
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct PqPoolBuilder;

impl<Node, Evm> PoolBuilder<Node, Evm> for PqPoolBuilder
where
    Node: FullNodeTypes<
        Types: NodeTypes<
            ChainSpec: EthereumHardforks,
            Primitives = PqPrimitives,
        >,
    >,
    Evm: ConfigureEvm<Primitives = PqPrimitives> + Clone + 'static,
{
    type Pool = PqTransactionPool<Node::Provider>;

    async fn build_pool(
        self,
        ctx: &BuilderContext<Node>,
        _evm_config: Evm,
    ) -> eyre::Result<Self::Pool> {
        let pool_config = ctx.pool_config();

        let blob_store = DiskFileBlobStore::open(
            ctx.config().datadir().blobstore(),
            Default::default(),
        )?;

        let validator = PqPoolValidator::new(ctx.provider().clone());

        let pool = reth_transaction_pool::Pool::new(
            validator,
            CoinbaseTipOrdering::default(),
            blob_store,
            pool_config,
        );

        info!(target: "reth::cli", "PQ transaction pool initialized (with state validation)");

        Ok(pool)
    }
}

// ─── PqNetworkBuilder ────────────────────────────────────────────────────────

/// Network builder for the PQ node.
///
/// Reuses the standard Ethereum networking stack — the P2P protocol is
/// transaction-type agnostic at this layer.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct PqNetworkBuilder;

impl<Node, Pool> NetworkBuilder<Node, Pool> for PqNetworkBuilder
where
    Node: FullNodeTypes<Types: NodeTypes<ChainSpec: reth_ethereum_forks::Hardforks>>,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<Node::Types>>>
        + Unpin
        + 'static,
{
    type Network =
        NetworkHandle<BasicNetworkPrimitives<PrimitivesTy<Node::Types>, PoolPooledTx<Pool>>>;

    async fn build_network(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<Self::Network> {
        let network = ctx.network_builder().await?;
        let handle = ctx.start_network(network, pool);
        info!(target: "reth::cli", enode=%handle.local_node_record(), "PQ P2P networking initialized");
        Ok(handle)
    }
}

// ─── PqConsensusBuilder ──────────────────────────────────────────────────────

/// Consensus builder for the PQ node.
///
/// Wraps `EthBeaconConsensus` with [`PqPoaConsensus`] for ML-DSA-65 seal
/// verification. When `PoA` is configured (via [`reth_pq_poa::set_validator_set`]),
/// the consensus validates seals in `extra_data`. Otherwise, it passes through.
///
/// Sets `max_extra_data_size = 3309` to accommodate the ML-DSA-65 seal.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct PqConsensusBuilder;

impl<Node> ConsensusBuilder<Node> for PqConsensusBuilder
where
    Node: FullNodeTypes<
        Types: NodeTypes<
            ChainSpec: EthChainSpec + EthereumHardforks,
            Primitives = PqPrimitives,
        >,
    >,
{
    type Consensus = Arc<reth_pq_poa::PqPoaConsensus<EthBeaconConsensus<<Node::Types as NodeTypes>::ChainSpec>>>;

    async fn build_consensus(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::Consensus> {
        // Allow 3309-byte extra_data for ML-DSA-65 seals
        let eth_consensus = EthBeaconConsensus::new(ctx.chain_spec())
            .with_max_extra_data_size(3309);

        let poa_consensus = if let Some(vs) = reth_pq_poa::get_validator_set() {
            info!(target: "reth::cli", validators = vs.len(), "PQ PoA consensus enabled — verifying ML-DSA-65 seals");
            reth_pq_poa::PqPoaConsensus::new(eth_consensus, vs.clone())
        } else {
            info!(target: "reth::cli", "PQ consensus initialized (no PoA seal verification)");
            reth_pq_poa::PqPoaConsensus::passthrough(eth_consensus)
        };

        Ok(Arc::new(poa_consensus))
    }
}

// ─── PqPayloadBuilderComponent ───────────────────────────────────────────────

/// Payload builder component for the PQ node.
///
/// Implements [`PayloadBuilderBuilder`] — constructs a [`PqPayloadBuilder`]
/// that builds blocks containing PQ (ML-DSA-65) transactions.
///
/// Designed to be used with [`BasicPayloadServiceBuilder`]:
/// ```ignore
/// BasicPayloadServiceBuilder::new(PqPayloadBuilderComponent)
/// ```
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct PqPayloadBuilderComponent;

impl<Node, Pool> reth_node_builder::components::PayloadBuilderBuilder<Node, Pool, PqEvmConfig>
    for PqPayloadBuilderComponent
where
    Node: FullNodeTypes<
        Types: NodeTypes<
            ChainSpec: EthereumHardforks,
            Primitives = PqPrimitives,
            Payload = PqEngineTypes,
        >,
    >,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<Node::Types>>>
        + Unpin
        + 'static,
{
    type PayloadBuilder = payload::PqPayloadBuilder<Pool, Node::Provider, PqEvmConfig>;

    async fn build_payload_builder(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
        evm_config: PqEvmConfig,
    ) -> eyre::Result<Self::PayloadBuilder> {
        // Use None so the payload builder follows the parent block's gas limit
        // (respecting the 1/1024 EIP-1559 adjustment rule).
        Ok(payload::PqPayloadBuilder::new(
            ctx.provider().clone(),
            pool,
            evm_config,
            None,
        ))
    }
}

// ─── PqEngineValidatorBuilder ────────────────────────────────────────────────

/// Builder for [`PqEngineValidator`](engine::PqEngineValidator).
///
/// Implements [`PayloadValidatorBuilder`] for the PQ node. Like
/// `EthereumEngineValidatorBuilder` but constrained to `Primitives = PqPrimitives`.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct PqEngineValidatorBuilder;

impl<Node, Types> PayloadValidatorBuilder<Node> for PqEngineValidatorBuilder
where
    Types: NodeTypes<
        ChainSpec: reth_ethereum_forks::Hardforks + EthereumHardforks + Clone + 'static,
        Payload: EngineTypes<ExecutionData = alloy_rpc_types_engine::ExecutionData>
            + PayloadTypes<PayloadAttributes = EthPayloadAttributes>,
        Primitives = PqPrimitives,
    >,
    Node: reth_node_api::FullNodeComponents<Types = Types>,
{
    type Validator = engine::PqEngineValidator<Types::ChainSpec>;

    async fn build(self, ctx: &AddOnsContext<'_, Node>) -> eyre::Result<Self::Validator> {
        Ok(engine::PqEngineValidator::new(ctx.config.chain.clone()))
    }
}

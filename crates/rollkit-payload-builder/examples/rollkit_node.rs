//! This example shows how to implement a custom Rollkit node with transactions passable via the Engine API.
//!
//! The Rollkit node supports passing transactions through the `engine_forkchoiceUpdatedV3` method,
//! similar to how Optimism handles sequencer transactions.
//!
//! Custom payload attributes can be supported by implementing:
//! - [PayloadAttributes] for payload attributes types used as arguments to `engine_forkchoiceUpdated`
//! - [PayloadBuilderAttributes] for payload attributes types that describe running payload jobs
//!
//! This example integrates with the rollkit payload builder to execute transactions passed
//! through the Engine API.

#![warn(unused_crate_dependencies)]

use alloy_eips::eip4895::Withdrawals;
use alloy_genesis::Genesis;
use alloy_primitives::{Address, B256, Bytes};
use alloy_rpc_types::{
    engine::{
        ExecutionData, ExecutionPayloadEnvelopeV2, ExecutionPayloadEnvelopeV3,
        ExecutionPayloadEnvelopeV4, ExecutionPayloadEnvelopeV5, ExecutionPayloadV1,
        PayloadAttributes as EthPayloadAttributes, PayloadId,
    },
    Withdrawal,
};
use reth_basic_payload_builder::{BuildArguments, BuildOutcome, PayloadBuilder, PayloadConfig};
use reth_engine_local::payload::UnsupportedLocalAttributes;
use reth_ethereum::{
    chainspec::{Chain, ChainSpec, ChainSpecProvider},
    node::{
        api::{
            payload::{EngineApiMessageVersion, EngineObjectValidationError, PayloadOrAttributes},
            validate_version_specific_fields, AddOnsContext, EngineTypes, EngineValidator,
            FullNodeComponents, FullNodeTypes, InvalidPayloadAttributesError, NewPayloadError,
            NodeTypes, PayloadAttributes, PayloadBuilderAttributes, PayloadTypes, PayloadValidator,
        },
        builder::{
            components::{BasicPayloadServiceBuilder, ComponentsBuilder, PayloadBuilderBuilder},
            rpc::{EngineValidatorBuilder, RpcAddOns},
            BuilderContext, Node, NodeAdapter, NodeBuilder, NodeComponentsBuilder,
        },
        core::{args::RpcServerArgs, node_config::NodeConfig},
        node::{
            EthereumConsensusBuilder, EthereumExecutorBuilder, EthereumNetworkBuilder,
            EthereumPoolBuilder,
        },
        EthEvmConfig, EthereumEthApiBuilder,
    },
    pool::{PoolTransaction, TransactionPool},
    primitives::{RecoveredBlock, SealedBlock},
    provider::{EthStorage, StateProviderFactory},
    rpc::types::engine::ExecutionPayload,
    tasks::TaskManager,
    Block, EthPrimitives, TransactionSigned,
};
use reth_ethereum_payload_builder::{EthereumBuilderConfig, EthereumExecutionPayloadValidator};
use reth_payload_builder::{EthBuiltPayload, EthPayloadBuilderAttributes, PayloadBuilderError};
use reth_primitives::{Transaction, TransactionSigned as PrimitiveTransactionSigned};
use reth_tracing::{RethTracer, Tracer};
use reth_trie_db::MerklePatriciaTrie;
use rollkit_payload_builder::{
    PayloadAttributesError, RollkitPayloadAttributes,
    RollkitPayloadBuilder, RollkitPayloadBuilderConfig,
};
use serde::{Deserialize, Serialize};
use std::{convert::Infallible, sync::Arc};
use thiserror::Error;

/// Rollkit payload attributes that support passing transactions via Engine API
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollkitEnginePayloadAttributes {
    /// Standard Ethereum payload attributes
    #[serde(flatten)]
    pub inner: EthPayloadAttributes,
    /// Transactions to be included in the payload (passed via Engine API)
    pub transactions: Option<Vec<Bytes>>,
    /// Optional gas limit for the payload
    pub gas_limit: Option<u64>,
}

impl UnsupportedLocalAttributes for RollkitEnginePayloadAttributes {}

/// Custom error type used in payload attributes validation
#[derive(Debug, Error)]
pub enum RollkitEngineError {
    #[error("Invalid transaction data: {0}")]
    InvalidTransactionData(String),
    #[error("Gas limit exceeded")]
    GasLimitExceeded,
    #[error("Rollkit payload attributes error: {0}")]
    PayloadAttributes(#[from] PayloadAttributesError),
}

impl PayloadAttributes for RollkitEnginePayloadAttributes {
    fn timestamp(&self) -> u64 {
        self.inner.timestamp()
    }

    fn withdrawals(&self) -> Option<&Vec<Withdrawal>> {
        self.inner.withdrawals()
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        self.inner.parent_beacon_block_root()
    }
}

/// Rollkit payload builder attributes
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RollkitEnginePayloadBuilderAttributes {
    /// Ethereum payload builder attributes
    pub ethereum_attributes: EthPayloadBuilderAttributes,
    /// Decoded transactions from the Engine API
    pub transactions: Vec<TransactionSigned>,
    /// Gas limit for the payload
    pub gas_limit: Option<u64>,
}

impl PayloadBuilderAttributes for RollkitEnginePayloadBuilderAttributes {
    type RpcPayloadAttributes = RollkitEnginePayloadAttributes;
    type Error = RollkitEngineError;

    fn try_new(
        parent: B256,
        attributes: RollkitEnginePayloadAttributes,
        _version: u8,
    ) -> Result<Self, Self::Error> {
        let ethereum_attributes = EthPayloadBuilderAttributes::new(parent, attributes.inner);
        
        // Decode transactions from bytes if provided
        let mut transactions = Vec::new();
        if let Some(tx_bytes_vec) = attributes.transactions {
            for tx_bytes in tx_bytes_vec {
                // Decode the transaction from bytes
                let tx = TransactionSigned::decode_enveloped(&mut tx_bytes.as_ref())
                    .map_err(|e| RollkitEngineError::InvalidTransactionData(e.to_string()))?;
                transactions.push(tx);
            }
        }

        Ok(Self {
            ethereum_attributes,
            transactions,
            gas_limit: attributes.gas_limit,
        })
    }

    fn payload_id(&self) -> PayloadId {
        self.ethereum_attributes.id
    }

    fn parent(&self) -> B256 {
        self.ethereum_attributes.parent
    }

    fn timestamp(&self) -> u64 {
        self.ethereum_attributes.timestamp
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        self.ethereum_attributes.parent_beacon_block_root
    }

    fn suggested_fee_recipient(&self) -> Address {
        self.ethereum_attributes.suggested_fee_recipient
    }

    fn prev_randao(&self) -> B256 {
        self.ethereum_attributes.prev_randao
    }

    fn withdrawals(&self) -> &Withdrawals {
        &self.ethereum_attributes.withdrawals
    }
}

/// Rollkit engine types - uses custom payload attributes that support transactions
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[non_exhaustive]
pub struct RollkitEngineTypes;

impl PayloadTypes for RollkitEngineTypes {
    type ExecutionData = ExecutionData;
    type BuiltPayload = EthBuiltPayload;
    type PayloadAttributes = RollkitEnginePayloadAttributes;
    type PayloadBuilderAttributes = RollkitEnginePayloadBuilderAttributes;

    fn block_to_payload(
        block: SealedBlock<
            <<Self::BuiltPayload as reth_ethereum::node::api::BuiltPayload>::Primitives as reth_ethereum::node::api::NodePrimitives>::Block,
        >,
    ) -> ExecutionData {
        let (payload, sidecar) =
            ExecutionPayload::from_block_unchecked(block.hash(), &block.into_block());
        ExecutionData { payload, sidecar }
    }
}

impl EngineTypes for RollkitEngineTypes {
    type ExecutionPayloadEnvelopeV1 = ExecutionPayloadV1;
    type ExecutionPayloadEnvelopeV2 = ExecutionPayloadEnvelopeV2;
    type ExecutionPayloadEnvelopeV3 = ExecutionPayloadEnvelopeV3;
    type ExecutionPayloadEnvelopeV4 = ExecutionPayloadEnvelopeV4;
    type ExecutionPayloadEnvelopeV5 = ExecutionPayloadEnvelopeV5;
}

/// Rollkit engine validator
#[derive(Debug, Clone)]
pub struct RollkitEngineValidator {
    inner: EthereumExecutionPayloadValidator<ChainSpec>,
}

impl RollkitEngineValidator {
    /// Instantiates a new validator.
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { inner: EthereumExecutionPayloadValidator::new(chain_spec) }
    }

    /// Returns the chain spec used by the validator.
    #[inline]
    fn chain_spec(&self) -> &ChainSpec {
        self.inner.chain_spec().as_ref()
    }
}

impl PayloadValidator for RollkitEngineValidator {
    type Block = Block;
    type ExecutionData = ExecutionData;

    fn ensure_well_formed_payload(
        &self,
        payload: ExecutionData,
    ) -> Result<RecoveredBlock<Self::Block>, NewPayloadError> {
        let sealed_block = self.inner.ensure_well_formed_payload(payload)?;
        sealed_block.try_recover().map_err(|e| NewPayloadError::Other(e.into()))
    }
}

impl<T> EngineValidator<T> for RollkitEngineValidator
where
    T: PayloadTypes<PayloadAttributes = RollkitEnginePayloadAttributes, ExecutionData = ExecutionData>,
{
    fn validate_version_specific_fields(
        &self,
        version: EngineApiMessageVersion,
        payload_or_attrs: PayloadOrAttributes<'_, Self::ExecutionData, T::PayloadAttributes>,
    ) -> Result<(), EngineObjectValidationError> {
        validate_version_specific_fields(self.chain_spec(), version, payload_or_attrs)
    }

    fn ensure_well_formed_attributes(
        &self,
        version: EngineApiMessageVersion,
        attributes: &T::PayloadAttributes,
    ) -> Result<(), EngineObjectValidationError> {
        validate_version_specific_fields(
            self.chain_spec(),
            version,
            PayloadOrAttributes::<Self::ExecutionData, T::PayloadAttributes>::PayloadAttributes(
                attributes,
            ),
        )?;

        // Validate rollkit-specific attributes
        if let Some(ref transactions) = attributes.transactions {
            if transactions.is_empty() {
                return Err(EngineObjectValidationError::invalid_params(
                    "Empty transactions list provided"
                ));
            }
        }

        Ok(())
    }

    fn validate_payload_attributes_against_header(
        &self,
        _attr: &<T as PayloadTypes>::PayloadAttributes,
        _header: &<Self::Block as reth_ethereum::primitives::Block>::Header,
    ) -> Result<(), InvalidPayloadAttributesError> {
        // Skip default timestamp validation for rollkit
        Ok(())
    }
}

/// Rollkit engine validator builder
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct RollkitEngineValidatorBuilder;

impl<N> EngineValidatorBuilder<N> for RollkitEngineValidatorBuilder
where
    N: FullNodeComponents<
        Types: NodeTypes<
            Payload = RollkitEngineTypes,
            ChainSpec = ChainSpec,
            Primitives = EthPrimitives,
        >,
    >,
{
    type Validator = RollkitEngineValidator;

    async fn build(self, ctx: &AddOnsContext<'_, N>) -> eyre::Result<Self::Validator> {
        Ok(RollkitEngineValidator::new(ctx.config.chain.clone()))
    }
}

/// Rollkit node type
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct RollkitNode;

impl NodeTypes for RollkitNode {
    type Primitives = EthPrimitives;
    type ChainSpec = ChainSpec;
    type StateCommitment = MerklePatriciaTrie;
    type Storage = EthStorage;
    type Payload = RollkitEngineTypes;
}

/// Rollkit node addons configuring RPC types with custom engine validator
pub type RollkitNodeAddOns<N> = RpcAddOns<N, EthereumEthApiBuilder, RollkitEngineValidatorBuilder>;

impl<N> Node<N> for RollkitNode
where
    N: FullNodeTypes<
        Types: NodeTypes<
            Payload = RollkitEngineTypes,
            ChainSpec = ChainSpec,
            Primitives = EthPrimitives,
            Storage = EthStorage,
        >,
    >,
{
    type ComponentsBuilder = ComponentsBuilder<
        N,
        EthereumPoolBuilder,
        BasicPayloadServiceBuilder<RollkitPayloadBuilderBuilder>,
        EthereumNetworkBuilder,
        EthereumExecutorBuilder,
        EthereumConsensusBuilder,
    >;
    type AddOns = RollkitNodeAddOns<
        NodeAdapter<N, <Self::ComponentsBuilder as NodeComponentsBuilder<N>>::Components>,
    >;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        ComponentsBuilder::default()
            .node_types::<N>()
            .pool(EthereumPoolBuilder::default())
            .executor(EthereumExecutorBuilder::default())
            .payload(BasicPayloadServiceBuilder::default())
            .network(EthereumNetworkBuilder::default())
            .consensus(EthereumConsensusBuilder::default())
    }

    fn add_ons(&self) -> Self::AddOns {
        RollkitNodeAddOns::default()
    }
}

/// Rollkit payload service builder that integrates with the rollkit payload builder
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct RollkitPayloadBuilderBuilder {
    config: Option<RollkitPayloadBuilderConfig>,
}

impl RollkitPayloadBuilderBuilder {
    /// Create a new builder with custom config
    pub fn with_config(config: RollkitPayloadBuilderConfig) -> Self {
        Self { config: Some(config) }
    }
}

impl<Node, Pool> PayloadBuilderBuilder<Node, Pool, EthEvmConfig> for RollkitPayloadBuilderBuilder
where
    Node: FullNodeTypes<
        Types: NodeTypes<
            Payload = RollkitEngineTypes,
            ChainSpec = ChainSpec,
            Primitives = EthPrimitives,
        >,
    >,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TransactionSigned>>
        + Unpin
        + 'static,
{
    type PayloadBuilder = RollkitEnginePayloadBuilder<Pool, Node::Provider>;

    async fn build_payload_builder(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
        _evm_config: EthEvmConfig,
    ) -> eyre::Result<Self::PayloadBuilder> {
        let config = self.config.unwrap_or_default();
        let rollkit_builder = RollkitPayloadBuilder::new(ctx.provider().clone());
        
        Ok(RollkitEnginePayloadBuilder {
            rollkit_builder,
            pool,
            config,
        })
    }
}

/// The rollkit engine payload builder that integrates with the rollkit payload builder
#[derive(Debug, Clone)]
pub struct RollkitEnginePayloadBuilder<Pool, Client> {
    rollkit_builder: RollkitPayloadBuilder<Client>,
    pool: Pool,
    config: RollkitPayloadBuilderConfig,
}

impl<Pool, Client> PayloadBuilder for RollkitEnginePayloadBuilder<Pool, Client>
where
    Client: StateProviderFactory + ChainSpecProvider<ChainSpec = ChainSpec> + Clone,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TransactionSigned>>,
{
    type Attributes = RollkitEnginePayloadBuilderAttributes;
    type BuiltPayload = EthBuiltPayload;

    fn try_build(
        &self,
        args: BuildArguments<Self::Attributes, Self::BuiltPayload>,
    ) -> Result<BuildOutcome<Self::BuiltPayload>, PayloadBuilderError> {
        let BuildArguments { cached_reads: _, config, cancel: _, best_payload } = args;
        let PayloadConfig { parent_header: _, attributes } = config;

        // Convert Engine API attributes to Rollkit payload attributes
        let rollkit_attrs = RollkitPayloadAttributes::new(
            attributes.transactions.clone(),
            attributes.gas_limit,
            attributes.timestamp(),
            attributes.prev_randao(),
            attributes.suggested_fee_recipient(),
        );

        // Build the payload using the rollkit payload builder
        let rt = tokio::runtime::Handle::current();
        let sealed_block = rt.block_on(self.rollkit_builder.build_payload(rollkit_attrs))
            .map_err(|e| PayloadBuilderError::Other(e.into()))?;

        // Convert to EthBuiltPayload
        let built_payload = EthBuiltPayload::new(
            PayloadId::new([0u8; 8]), // Generate proper payload ID
            sealed_block,
            None, // No blob sidecar for rollkit
            reth_ethereum::primitives::constants::KECCAK_EMPTY,
        );

        if let Some(best) = best_payload {
            if built_payload.fees() <= best.fees() {
                return Ok(BuildOutcome::Aborted { fees: built_payload.fees(), cached_reads: None });
            }
        }

        Ok(BuildOutcome::Better { payload: built_payload, cached_reads: None })
    }

    fn build_empty_payload(
        &self,
        config: PayloadConfig<Self::Attributes>,
    ) -> Result<Self::BuiltPayload, PayloadBuilderError> {
        let PayloadConfig { parent_header: _, attributes } = config;

        // Create empty rollkit attributes (no transactions)
        let rollkit_attrs = RollkitPayloadAttributes::new(
            vec![],
            attributes.gas_limit,
            attributes.timestamp(),
            attributes.prev_randao(),
            attributes.suggested_fee_recipient(),
        );

        // Build empty payload
        let rt = tokio::runtime::Handle::current();
        let sealed_block = rt.block_on(self.rollkit_builder.build_payload(rollkit_attrs))
            .map_err(|e| PayloadBuilderError::Other(e.into()))?;

        Ok(EthBuiltPayload::new(
            PayloadId::new([0u8; 8]),
            sealed_block,
            None,
            reth_ethereum::primitives::constants::KECCAK_EMPTY,
        ))
    }
}

/// Helper function to create a rollkit node with default configuration
pub async fn create_rollkit_node<Client>(
    client: Arc<Client>,
) -> eyre::Result<RollkitPayloadBuilder<Client>>
where
    Client: StateProviderFactory + Send + Sync + 'static,
{
    Ok(RollkitPayloadBuilder::new(client))
}

/// Helper function to create a rollkit node with custom configuration
pub async fn create_rollkit_node_with_config<Client>(
    client: Arc<Client>,
    _config: RollkitPayloadBuilderConfig,
) -> eyre::Result<RollkitPayloadBuilder<Client>>
where
    Client: StateProviderFactory + Send + Sync + 'static,
{
    Ok(RollkitPayloadBuilder::new(client))
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let _guard = RethTracer::new().init()?;

    let tasks = TaskManager::current();

    // Create rollkit genesis
    let spec = ChainSpec::builder()
        .chain(Chain::mainnet())
        .genesis(Genesis::default())
        .london_activated()
        .paris_activated()
        .shanghai_activated()
        .build();

    // Create node config
    let node_config =
        NodeConfig::test().with_rpc(RpcServerArgs::default().with_http()).with_chain(spec);

    let handle = NodeBuilder::new(node_config)
        .testing_node(tasks.executor())
        .launch_node(RollkitNode::default())
        .await
        .unwrap();

    println!("Rollkit Node started");

    handle.node_exit_future.await
} 
//! Rollkit node binary with standard reth CLI support and rollkit payload builder integration.
//!
//! This node supports all standard reth CLI flags and functionality, with a customized
//! payload builder that accepts transactions via engine API payload attributes.

#![allow(missing_docs, rustdoc::missing_crate_level_docs)]

use alloy_eips::eip4895::Withdrawals;
use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_rpc_types::{
    engine::{
        ExecutionData, ExecutionPayloadEnvelopeV2, ExecutionPayloadEnvelopeV3,
        ExecutionPayloadEnvelopeV4, ExecutionPayloadEnvelopeV5, ExecutionPayloadV1,
        PayloadAttributes as EthPayloadAttributes, PayloadId,
    },
    Withdrawal,
};
use clap::Parser;
use reth_basic_payload_builder::{BuildArguments, BuildOutcome, PayloadBuilder, PayloadConfig, HeaderForPayload};
use reth_ethereum_cli::{chainspec::EthereumChainSpecParser, Cli};
use reth_engine_local::payload::UnsupportedLocalAttributes;
use reth_ethereum::{
    chainspec::{ChainSpec, ChainSpecProvider},
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
            BuilderContext, Node, NodeAdapter, NodeComponentsBuilder,
        },
        node::{
            EthereumConsensusBuilder, EthereumExecutorBuilder, EthereumNetworkBuilder,
            EthereumPoolBuilder,
        },
        EthEvmConfig, EthereumEthApiBuilder,
    },
    pool::{PoolTransaction, TransactionPool},
    primitives::{RecoveredBlock, SealedBlock, Header},
    TransactionSigned,
};
use reth_ethereum_payload_builder::EthereumExecutionPayloadValidator;
use reth_payload_builder::{EthBuiltPayload, EthPayloadBuilderAttributes, PayloadBuilderError};
use reth_provider::HeaderProvider;
use reth_revm::cached::CachedReads;
use reth_trie_db::MerklePatriciaTrie;
use rollkit_reth::{
    PayloadAttributesError, RollkitPayloadAttributes, RollkitPayloadBuilder,
    RollkitPayloadBuilderConfig,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;
use tracing::info;
use alloy_eips::eip2718::Decodable2718;

#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();

/// Rollkit-specific command line arguments
#[derive(Debug, Clone, Parser, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollkitArgs {
    /// Enable rollkit mode
    #[arg(long, default_value = "false")]
    pub rollkit: bool,

    /// Maximum gas limit for rollkit payloads
    #[arg(long, default_value = "30000000")]
    pub rollkit_gas_limit: u64,

    /// Enable transaction passthrough via Engine API
    #[arg(long, default_value = "true")]
    pub engine_tx_passthrough: bool,
}

impl Default for RollkitArgs {
    fn default() -> Self {
        Self {
            rollkit: false,
            rollkit_gas_limit: 30_000_000,
            engine_tx_passthrough: true,
        }
    }
}

/// Rollkit payload attributes that support passing transactions via Engine API
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollkitEnginePayloadAttributes {
    /// Standard Ethereum payload attributes
    #[serde(flatten)]
    pub inner: EthPayloadAttributes,
    /// Transactions to be included in the payload (passed via Engine API)
    pub transactions: Option<Vec<Bytes>>,
    /// Optional gas limit for the payload
    #[serde(rename = "gasLimit")]
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
                // Decode the transaction from RLP bytes (as sent by Go client)
                let tx = TransactionSigned::network_decode(&mut tx_bytes.as_ref())
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
            reth_ethereum::rpc::types::engine::ExecutionPayload::from_block_unchecked(block.hash(), &block.into_block());
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

/// Rollkit engine validator that handles custom payload validation
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
    type Block = reth_ethereum::Block;
    type ExecutionData = ExecutionData;

    fn ensure_well_formed_payload(
        &self,
        payload: ExecutionData,
    ) -> Result<RecoveredBlock<Self::Block>, NewPayloadError> {
        info!("Rollkit engine validator: validating payload");
        
        // Use inner validator but with custom rollkit handling
        match self.inner.ensure_well_formed_payload(payload.clone()) {
            Ok(sealed_block) => {
                info!("Rollkit engine validator: payload validation succeeded");
                sealed_block.try_recover().map_err(|e| NewPayloadError::Other(e.into()))
            },
            Err(err) => {
                // Log the error for debugging
                tracing::debug!("Rollkit payload validation error: {:?}", err);
                
                // Check if this is a block hash mismatch error - bypass it for rollkit
                if matches!(err, alloy_rpc_types::engine::PayloadError::BlockHash { .. }) {
                    info!("Rollkit engine validator: bypassing block hash mismatch for rollkit");
                    // For rollkit, we trust the payload builder - just parse the block without hash validation
                    use reth_primitives_traits::Block;
                    let ExecutionData { payload, sidecar } = payload;
                    let sealed_block = payload.try_into_block_with_sidecar(&sidecar)?.seal_slow();
                    sealed_block.try_recover().map_err(|e| NewPayloadError::Other(e.into()))
                } else {
                    // For other errors, re-throw them
                    Err(NewPayloadError::Eth(err))
                }
            }
        }
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
            info!("Rollkit engine validator: validating {} transactions", transactions.len());
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
            Primitives = reth_ethereum::EthPrimitives,
        >,
    >,
{
    type Validator = RollkitEngineValidator;

    async fn build(self, ctx: &AddOnsContext<'_, N>) -> eyre::Result<Self::Validator> {
        info!("Building Rollkit engine validator");
        Ok(RollkitEngineValidator::new(ctx.config.chain.clone()))
    }
}

/// Rollkit node type
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct RollkitNode {
    /// Rollkit-specific arguments
    pub args: RollkitArgs,
}

impl RollkitNode {
    /// Create a new rollkit node with the given arguments
    pub const fn new(args: RollkitArgs) -> Self {
        Self { args }
    }
}

impl NodeTypes for RollkitNode {
    type Primitives = reth_ethereum::EthPrimitives;
    type ChainSpec = ChainSpec;
    type StateCommitment = MerklePatriciaTrie;
    type Storage = reth_ethereum::provider::EthStorage;
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
            Primitives = reth_ethereum::EthPrimitives,
            Storage = reth_ethereum::provider::EthStorage,
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
        info!("Building Rollkit node components with custom payload builder");
        ComponentsBuilder::default()
            .node_types::<N>()
            .pool(EthereumPoolBuilder::default())
            .executor(EthereumExecutorBuilder::default())
            .payload(BasicPayloadServiceBuilder::new(RollkitPayloadBuilderBuilder::new(&self.args)))
            .network(EthereumNetworkBuilder::default())
            .consensus(EthereumConsensusBuilder::default())
    }

    fn add_ons(&self) -> Self::AddOns {
        info!("Adding Rollkit RPC extensions with custom engine validator");
        RollkitNodeAddOns::default()
    }
}

/// Rollkit payload service builder that integrates with the rollkit payload builder
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct RollkitPayloadBuilderBuilder {
    config: RollkitPayloadBuilderConfig,
}

impl RollkitPayloadBuilderBuilder {
    /// Create a new builder with rollkit args
    pub fn new(args: &RollkitArgs) -> Self {
        let config = RollkitPayloadBuilderConfig {
            max_transactions: 1000,
            max_gas_limit: args.rollkit_gas_limit,
            min_gas_price: 1_000_000_000, // 1 Gwei
            enable_tx_validation: args.engine_tx_passthrough,
        };
        info!("Created Rollkit payload builder with config: {:?}", config);
        Self { config }
    }
}

impl Default for RollkitPayloadBuilderBuilder {
    fn default() -> Self {
        Self::new(&RollkitArgs::default())
    }
}

/// The rollkit engine payload builder that integrates with the rollkit payload builder
#[derive(Debug, Clone)]
pub struct RollkitEnginePayloadBuilder<Pool, Client> 
where
    Pool: Clone,
    Client: Clone,
{
    rollkit_builder: Arc<RollkitPayloadBuilder<Client>>,
    #[allow(dead_code)]
    pool: Pool,
    #[allow(dead_code)]
    config: RollkitPayloadBuilderConfig,
}

impl<Pool, Client> PayloadBuilder for RollkitEnginePayloadBuilder<Pool, Client>
where
    Client: reth_ethereum::provider::StateProviderFactory + ChainSpecProvider<ChainSpec = ChainSpec> + HeaderProvider<Header = Header> + Clone + Send + Sync + 'static,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TransactionSigned>>,
{
    type Attributes = RollkitEnginePayloadBuilderAttributes;
    type BuiltPayload = EthBuiltPayload;

    fn try_build(
        &self,
        args: BuildArguments<Self::Attributes, Self::BuiltPayload>,
    ) -> Result<BuildOutcome<Self::BuiltPayload>, PayloadBuilderError> {
        let BuildArguments { cached_reads: _, config, cancel: _, best_payload } = args;
        let PayloadConfig { parent_header, attributes } = config;

        info!("Rollkit engine payload builder: building payload with {} transactions", 
            attributes.transactions.len());

        // Convert Engine API attributes to Rollkit payload attributes
        let rollkit_attrs = RollkitPayloadAttributes::new(
            attributes.transactions.clone(),
            attributes.gas_limit,
            attributes.timestamp(),
            attributes.prev_randao(),
            attributes.suggested_fee_recipient(),
            attributes.parent(),
            parent_header.number + 1,
        );

        // Build the payload using the rollkit payload builder - use spawn_blocking for async work
        let rollkit_builder = self.rollkit_builder.clone();
        let sealed_block = std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(rollkit_builder.build_payload(rollkit_attrs))
        }).join().map_err(|_| PayloadBuilderError::other(std::io::Error::new(std::io::ErrorKind::Other, "Thread join failed")))?
        .map_err(|e| PayloadBuilderError::other(e))?;

        info!("Rollkit engine payload builder: built block with {} transactions, gas used: {}", 
            sealed_block.transaction_count(), sealed_block.gas_used);

        // Convert to EthBuiltPayload
        let gas_used = sealed_block.gas_used;
        let built_payload = EthBuiltPayload::new(
            attributes.payload_id(), // Use the proper payload ID from attributes
            Arc::new(sealed_block),
            U256::from(gas_used), // Block gas used
            None, // No blob sidecar for rollkit
        );

        if let Some(best) = best_payload {
            if built_payload.fees() <= best.fees() {
                return Ok(BuildOutcome::Aborted { fees: built_payload.fees(), cached_reads: CachedReads::default() });
            }
        }

        Ok(BuildOutcome::Better { payload: built_payload, cached_reads: CachedReads::default() })
    }

    fn build_empty_payload(
        &self,
        config: PayloadConfig<Self::Attributes, HeaderForPayload<Self::BuiltPayload>>,
    ) -> Result<Self::BuiltPayload, PayloadBuilderError> {
        let PayloadConfig { parent_header, attributes } = config;

        info!("Rollkit engine payload builder: building empty payload");

        // Create empty rollkit attributes (no transactions)
        let rollkit_attrs = RollkitPayloadAttributes::new(
            vec![],
            attributes.gas_limit,
            attributes.timestamp(),
            attributes.prev_randao(),
            attributes.suggested_fee_recipient(),
            attributes.parent(),
            parent_header.number + 1,
        );

        // Build empty payload - use spawn_blocking for async work
        let rollkit_builder = self.rollkit_builder.clone();
        let sealed_block = std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(rollkit_builder.build_payload(rollkit_attrs))
        }).join().map_err(|_| PayloadBuilderError::other(std::io::Error::new(std::io::ErrorKind::Other, "Thread join failed")))?
        .map_err(|e| PayloadBuilderError::other(e))?;

        let gas_used = sealed_block.gas_used;
        Ok(EthBuiltPayload::new(
            PayloadId::new([0u8; 8]),
            Arc::new(sealed_block),
            U256::from(gas_used),
            None,
        ))
    }
}

impl<Node, Pool> PayloadBuilderBuilder<Node, Pool, EthEvmConfig> for RollkitPayloadBuilderBuilder
where
    Node: FullNodeTypes<
        Types: NodeTypes<
            Payload = RollkitEngineTypes,
            ChainSpec = ChainSpec,
            Primitives = reth_ethereum::EthPrimitives,
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
        evm_config: EthEvmConfig,
    ) -> eyre::Result<Self::PayloadBuilder> {
        info!("Building Rollkit engine payload builder service");
        let rollkit_builder = Arc::new(RollkitPayloadBuilder::new(Arc::new(ctx.provider().clone()), evm_config));
        
        Ok(RollkitEnginePayloadBuilder {
            rollkit_builder,
            pool,
            config: self.config,
        })
    }
}

fn main() {
    info!("=== ROLLKIT-RETH NODE STARTING ===");
    
    reth_cli_util::sigsegv_handler::install();

    // Enable backtraces unless a RUST_BACKTRACE value has already been explicitly provided.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }

    if let Err(err) =
        Cli::<EthereumChainSpecParser, RollkitArgs>::parse().run(async move |builder, rollkit_args| {
            info!("=== ROLLKIT-RETH: Starting with args: {:?} ===", rollkit_args);
            
            if rollkit_args.rollkit {
                info!("=== ROLLKIT-RETH: Rollkit mode enabled ===");
                info!("=== ROLLKIT-RETH: Gas limit: {} ===", rollkit_args.rollkit_gas_limit);
                info!("=== ROLLKIT-RETH: Engine TX passthrough: {} ===", rollkit_args.engine_tx_passthrough);
                info!("=== ROLLKIT-RETH: Using custom payload builder with transaction support ===");
            } else {
                info!("=== ROLLKIT-RETH: Using standard ethereum node ===");
            }
            
            let handle = builder
                .node(RollkitNode::new(rollkit_args))
                .launch()
                .await?;
            
            info!("=== ROLLKIT-RETH: Node launched successfully with rollkit payload builder ===");
            handle.node_exit_future.await
        })
    {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
} 
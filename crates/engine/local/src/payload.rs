//! The implementation of the [`PayloadAttributesBuilder`] for the
//! [`LocalMiner`](super::LocalMiner).

use alloy_primitives::{Address, B256};
use reth_chainspec::EthereumHardforks;
use reth_ethereum_engine_primitives::EthPayloadAttributes;
use reth_node_api::{AddOnsContext, FullNodeComponents};
use reth_payload_primitives::PayloadAttributesBuilder;
use std::{marker::PhantomData, sync::Arc};

/// The attributes builder for local Ethereum payload.
#[derive(Debug)]
#[non_exhaustive]
pub struct LocalPayloadAttributesBuilder<ChainSpec> {
    chain_spec: Arc<ChainSpec>,
}

impl<ChainSpec> LocalPayloadAttributesBuilder<ChainSpec> {
    /// Creates a new instance of the builder.
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec }
    }
}

impl<ChainSpec> PayloadAttributesBuilder<EthPayloadAttributes>
    for LocalPayloadAttributesBuilder<ChainSpec>
where
    ChainSpec: Send + Sync + EthereumHardforks + 'static,
{
    fn build(&self, timestamp: u64) -> EthPayloadAttributes {
        EthPayloadAttributes {
            timestamp,
            prev_randao: B256::random(),
            suggested_fee_recipient: Address::random(),
            withdrawals: self
                .chain_spec
                .is_shanghai_active_at_timestamp(timestamp)
                .then(Default::default),
            parent_beacon_block_root: self
                .chain_spec
                .is_cancun_active_at_timestamp(timestamp)
                .then(B256::random),
        }
    }
}

#[cfg(feature = "op")]
impl<ChainSpec> PayloadAttributesBuilder<op_alloy_rpc_types_engine::OpPayloadAttributes>
    for LocalPayloadAttributesBuilder<ChainSpec>
where
    ChainSpec: Send + Sync + EthereumHardforks + 'static,
{
    fn build(&self, timestamp: u64) -> op_alloy_rpc_types_engine::OpPayloadAttributes {
        op_alloy_rpc_types_engine::OpPayloadAttributes {
            payload_attributes: self.build(timestamp),
            // Add dummy system transaction
            transactions: Some(vec![
                reth_optimism_chainspec::constants::TX_SET_L1_BLOCK_OP_MAINNET_BLOCK_124665056
                    .into(),
            ]),
            no_tx_pool: None,
            gas_limit: None,
            eip_1559_params: None,
        }
    }
}

/// Provides local payload attributes builder functionality
pub trait LocalPayloadAttributesAddOn<N: FullNodeComponents> {
    /// The payload attributes type this builder produces
    type PayloadAttributes;

    /// Creates a local payload attributes builder
    fn local_payload_attributes_builder(
        &self,
        ctx: &AddOnsContext<'_, N>,
    ) -> eyre::Result<impl PayloadAttributesBuilder<Self::PayloadAttributes>>;
}

impl PayloadAttributesBuilder<()> for LocalPayloadAttributesBuilder<()> {
    fn build(&self, _: u64) {
        unreachable!("Unit type builder should never be used")
    }
}

/// A default implementation for nodes that don't provide local payload building.
///
/// It returns an error when asked to build payload attributes,
/// indicating that the feature is not available.
///
/// Useful as a fallback or when local building capabilities are not needed.
#[derive(Debug)]
pub struct UnsupportedLocalPayloadAttributesAddOn;

impl<N: FullNodeComponents> LocalPayloadAttributesAddOn<N>
    for UnsupportedLocalPayloadAttributesAddOn
{
    type PayloadAttributes = ();

    fn local_payload_attributes_builder(
        &self,
        _ctx: &AddOnsContext<'_, N>,
    ) -> eyre::Result<impl PayloadAttributesBuilder<Self::PayloadAttributes>> {
        Err(eyre::eyre!("Not supported"))
            .map(|_: ()| LocalPayloadAttributesBuilder::new(Arc::new(())))
    }
}

/// A builder implementation that indicates payload building is not configured.
///
/// Helpful for use cases where local payload building is not supported or not implemented.
#[derive(Debug, Clone)]
pub struct UnsupportedPayloadAttributesBuilder<T> {
    _phantom: PhantomData<T>,
}

impl<T: Send + Sync + 'static> PayloadAttributesBuilder<T>
    for UnsupportedPayloadAttributesBuilder<T>
{
    fn build(&self, _timestamp: u64) -> T {
        unreachable!(
            "UnsupportedPayloadAttributesBuilder should never be used to build payload attributes"
        )
    }
}

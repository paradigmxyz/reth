//! The implementation of the [`PayloadAttributesBuilder`] for the
//! [`LocalEngineService`](super::service::LocalEngineService).

use alloy_primitives::{Address, B256};
use reth_chainspec::EthereumHardforks;
use reth_ethereum_engine_primitives::EthPayloadAttributes;
use reth_payload_primitives::PayloadAttributesBuilder;
use std::sync::Arc;

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
            withdrawals: if self.chain_spec.is_shanghai_active_at_timestamp(timestamp) {
                Some(Default::default())
            } else {
                None
            },
            parent_beacon_block_root: if self.chain_spec.is_cancun_active_at_timestamp(timestamp) {
                Some(B256::random())
            } else {
                None
            },
        }
    }
}

#[cfg(feature = "optimism")]
impl<ChainSpec> PayloadAttributesBuilder<op_alloy_rpc_types_engine::OpPayloadAttributes>
    for LocalPayloadAttributesBuilder<ChainSpec>
where
    ChainSpec: Send + Sync + EthereumHardforks + 'static,
{
    fn build(&self, timestamp: u64) -> op_alloy_rpc_types_engine::OpPayloadAttributes {
        op_alloy_rpc_types_engine::OpPayloadAttributes {
            payload_attributes: self.build(timestamp),
            transactions: None,
            no_tx_pool: None,
            gas_limit: None,
            eip_1559_params: None,
        }
    }
}

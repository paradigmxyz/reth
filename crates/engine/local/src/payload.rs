//! The implementation of the [`PayloadAttributesBuilder`] for the
//! [`LocalMiner`](super::LocalMiner).

use alloy_consensus::BlockHeader;
use alloy_primitives::{Address, B256};
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_ethereum_engine_primitives::EthPayloadAttributes;
use reth_payload_primitives::PayloadAttributesBuilder;
use reth_primitives_traits::SealedHeader;
use std::sync::Arc;

/// The attributes builder for local Ethereum payload.
#[derive(Debug)]
#[non_exhaustive]
pub struct LocalPayloadAttributesBuilder<ChainSpec> {
    /// The chainspec
    pub chain_spec: Arc<ChainSpec>,

    /// Whether to enforce increasing timestamp.
    pub enforce_increasing_timestamp: bool,
}

impl<ChainSpec> LocalPayloadAttributesBuilder<ChainSpec> {
    /// Creates a new instance of the builder.
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec, enforce_increasing_timestamp: true }
    }

    /// Creates a new instance of the builder without enforcing increasing timestamps.
    pub fn without_increasing_timestamp(self) -> Self {
        Self { enforce_increasing_timestamp: false, ..self }
    }
}

impl<ChainSpec> PayloadAttributesBuilder<EthPayloadAttributes, ChainSpec::Header>
    for LocalPayloadAttributesBuilder<ChainSpec>
where
    ChainSpec: EthChainSpec + EthereumHardforks + 'static,
{
    fn build(&self, parent: &SealedHeader<ChainSpec::Header>) -> EthPayloadAttributes {
        let mut timestamp =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

        if self.enforce_increasing_timestamp {
            timestamp = std::cmp::max(parent.timestamp().saturating_add(1), timestamp);
        }

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
impl<ChainSpec>
    PayloadAttributesBuilder<op_alloy_rpc_types_engine::OpPayloadAttributes, ChainSpec::Header>
    for LocalPayloadAttributesBuilder<ChainSpec>
where
    ChainSpec: EthChainSpec + EthereumHardforks + 'static,
{
    fn build(
        &self,
        parent: &SealedHeader<ChainSpec::Header>,
    ) -> op_alloy_rpc_types_engine::OpPayloadAttributes {
        use alloy_primitives::B64;
        use reth_chainspec::BaseFeeParams;
        use std::env;
        /// Dummy system transaction for dev mode.
        /// OP Mainnet transaction at index 0 in block 124665056.
        ///
        /// <https://optimistic.etherscan.io/tx/0x312e290cf36df704a2217b015d6455396830b0ce678b860ebfcc30f41403d7b1>
        const TX_SET_L1_BLOCK_OP_MAINNET_BLOCK_124665056: [u8; 251] = alloy_primitives::hex!(
            "7ef8f8a0683079df94aa5b9cf86687d739a60a9b4f0835e520ec4d664e2e415dca17a6df94deaddeaddeaddeaddeaddeaddeaddeaddead00019442000000000000000000000000000000000000158080830f424080b8a4440a5e200000146b000f79c500000000000000040000000066d052e700000000013ad8a3000000000000000000000000000000000000000000000000000000003ef1278700000000000000000000000000000000000000000000000000000000000000012fdf87b89884a61e74b322bbcf60386f543bfae7827725efaaf0ab1de2294a590000000000000000000000006887246668a3b87f54deb3b94ba47a6f63f32985"
        );

        // Configure EIP-1559 parameters for dev mode. These can be overridden via environment
        // variables (OP_DEV_EIP1559_DENOMINATOR, OP_DEV_EIP1559_ELASTICITY, OP_DEV_GAS_LIMIT),
        // otherwise defaults from Optimism's BaseFeeParams are used. The parameters are encoded
        // as an 8-byte value (denominator + elasticity) required by Optimism's Jovian fork.
        let default_eip_1559_params = BaseFeeParams::optimism();
        let denominator = env::var("OP_DEV_EIP1559_DENOMINATOR")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(default_eip_1559_params.max_change_denominator as u32);
        let elasticity = env::var("OP_DEV_EIP1559_ELASTICITY")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(default_eip_1559_params.elasticity_multiplier as u32);
        let gas_limit = env::var("OP_DEV_GAS_LIMIT").ok().and_then(|v| v.parse::<u64>().ok());

        let mut eip1559_bytes = [0u8; 8];
        eip1559_bytes[0..4].copy_from_slice(&denominator.to_be_bytes());
        eip1559_bytes[4..8].copy_from_slice(&elasticity.to_be_bytes());
        let eip_1559_params = Some(B64::from(eip1559_bytes));

        op_alloy_rpc_types_engine::OpPayloadAttributes {
            payload_attributes: self.build(parent),
            transactions: Some(vec![TX_SET_L1_BLOCK_OP_MAINNET_BLOCK_124665056.into()]),
            no_tx_pool: None,
            gas_limit,
            eip_1559_params,
            min_base_fee: Some(0),
        }
    }
}

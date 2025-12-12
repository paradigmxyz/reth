//! Mantle-specific EVM environment configuration.
//!
//! This module provides Mantle chain support with hardfork awareness,
//! extending the base OP Stack configuration with Mantle-specific features
//! such as the Skadi hardfork.
//!
//! # Design
//!
//! The implementation follows the same pattern as `alloy-evm`:
//! - [`MantleEvmEnvInput`]: Encapsulates block environment parameters
//! - [`OpEvmConfig::for_mantle`]: Core method for EVM environment creation
//!
//! This ensures that Mantle-specific hardforks (like Skadi) are correctly
//! detected in all code paths (Engine API, RPC validation, block execution).

use alloy_consensus::{BlockHeader, Header};
use alloy_primitives::U256;
use op_alloy_rpc_types_engine::OpExecutionData;
use op_revm::OpSpecId;
use reth_chainspec::EthChainSpec;
use reth_evm::EvmEnv;
use reth_mantle_forks::MantleHardforks;
use reth_optimism_forks::OpHardforks;
use reth_primitives_traits::NodePrimitives;
use revm::{
    context::{BlockEnv, CfgEnv},
    context_interface::block::BlobExcessGasAndPrice,
    primitives::hardfork::SpecId,
};
use tracing::debug;

use crate::{config::OpNextBlockEnvAttributes, OpEvmConfig};

/// Input parameters for constructing EVM environment on Mantle chains.
///
/// This structure follows the same design pattern as `EvmEnvInput` from `alloy-evm`,
/// encapsulating block environment parameters with Mantle hardfork awareness.
///
/// The input can be constructed from various sources:
/// - [`MantleEvmEnvInput::from_block_header`]: From a block header
/// - [`MantleEvmEnvInput::for_next`]: For the next block (from parent + attributes)
/// - [`MantleEvmEnvInput::from_op_payload`]: From an execution payload
///
/// # Reference
/// Similar to `EvmEnvInput` in `alloy-evm/src/eth/env.rs`
pub(crate) struct MantleEvmEnvInput {
    pub(crate) timestamp: u64,
    pub(crate) number: u64,
    pub(crate) beneficiary: alloy_primitives::Address,
    pub(crate) gas_limit: u64,
    pub(crate) base_fee_per_gas: u64,
    pub(crate) difficulty: U256,
    pub(crate) mix_hash: Option<alloy_primitives::B256>,
    pub(crate) excess_blob_gas: Option<u64>,
}

impl MantleEvmEnvInput {
    /// Create input from a block header.
    ///
    /// Extracts all necessary EVM environment parameters from the given header.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let input = MantleEvmEnvInput::from_block_header(&header);
    /// let evm_env = config.for_mantle(input);
    /// ```
    pub(crate) fn from_block_header(header: &Header) -> Self {
        Self {
            timestamp: header.timestamp(),
            number: header.number(),
            beneficiary: header.beneficiary(),
            gas_limit: header.gas_limit(),
            base_fee_per_gas: header.base_fee_per_gas().unwrap_or_default(),
            difficulty: header.difficulty(),
            mix_hash: header.mix_hash(),
            excess_blob_gas: header.excess_blob_gas(),
        }
    }

    /// Create input for the next block from parent header and attributes.
    ///
    /// This is typically used when building a new block. The block number is
    /// automatically incremented from the parent, and difficulty is set to zero
    /// (as all OP Stack chains are post-merge).
    ///
    /// # Arguments
    ///
    /// * `parent` - The parent block header
    /// * `attributes` - Next block attributes (timestamp, gas limit, etc.)
    /// * `base_fee_per_gas` - Calculated base fee for the next block
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let base_fee = chain_spec.next_block_base_fee(parent, attributes.timestamp)?;
    /// let input = MantleEvmEnvInput::for_next(parent, &attributes, base_fee);
    /// let evm_env = config.for_mantle(input);
    /// ```
    pub(crate) fn for_next(
        parent: &Header,
        attributes: &OpNextBlockEnvAttributes,
        base_fee_per_gas: u64,
    ) -> Self {
        Self {
            timestamp: attributes.timestamp,
            number: parent.number() + 1,
            beneficiary: attributes.suggested_fee_recipient,
            gas_limit: attributes.gas_limit,
            base_fee_per_gas,
            difficulty: U256::ZERO,
            mix_hash: Some(attributes.prev_randao),
            excess_blob_gas: None,
        }
    }

    /// Create input from an OP execution payload.
    ///
    /// This is used when processing execution payloads from the Engine API.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let input = MantleEvmEnvInput::from_op_payload(&payload);
    /// let evm_env = config.for_mantle(input);
    /// ```
    pub(crate) fn from_op_payload(payload: &OpExecutionData) -> Self {
        Self {
            timestamp: payload.payload.timestamp(),
            number: payload.payload.block_number(),
            beneficiary: payload.payload.as_v1().fee_recipient,
            gas_limit: payload.payload.as_v1().gas_limit,
            base_fee_per_gas: payload.payload.as_v1().base_fee_per_gas.to::<u64>(),
            difficulty: payload.payload.as_v1().prev_randao.into(),
            mix_hash: Some(payload.payload.as_v1().prev_randao),
            excess_blob_gas: payload.payload.as_v3().map(|v| v.excess_blob_gas),
        }
    }
}

impl<ChainSpec, N, R, EvmFactory> OpEvmConfig<ChainSpec, N, R, EvmFactory>
where
    ChainSpec: EthChainSpec<Header = Header> + OpHardforks + MantleHardforks,
    N: NodePrimitives,
{
    /// Creates [`EvmEnv`] for Mantle chains with unified spec determination logic.
    ///
    /// This is the core method for EVM environment creation, ensuring all code paths
    /// use the same spec determination logic via [`MantleHardforks::revm_spec_at_timestamp`].
    ///
    /// # Mantle Hardfork Support
    ///
    /// Unlike the standard OP Stack implementation, this method:
    /// - Checks Mantle-specific hardforks (e.g., Skadi) first via the trait method
    /// - Falls back to standard OP Stack hardforks if no Mantle fork is active
    /// - Returns `OpSpecId::OSAKA` when Skadi is active
    ///
    /// # Code Paths Using This Method
    ///
    /// All EVM environment creation paths now use this method:
    /// - Engine API payloads: `evm_env_for_payload` → `for_mantle`
    /// - RPC validation: `evm_env` → `for_mantle`
    /// - Block execution: `evm_env` → `for_mantle`
    /// - Next block building: `next_evm_env` → `for_mantle`
    ///
    /// This ensures consistent hardfork detection across all scenarios.
    ///
    /// # Arguments
    ///
    /// * `input` - Block environment parameters encapsulated in [`MantleEvmEnvInput`]
    ///
    /// # Returns
    ///
    /// An [`EvmEnv`] configured with the appropriate [`OpSpecId`] for the given timestamp.
    ///
    /// # Design
    ///
    /// This follows the same pattern as `for_op` in `alloy-op-evm`, but with Mantle
    /// hardfork awareness. See [`MantleHardforks::revm_spec_at_timestamp`] for details
    /// on the spec determination logic.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let input = MantleEvmEnvInput::from_block_header(&header);
    /// let evm_env = config.for_mantle(input);
    /// // evm_env.cfg_env.spec == OpSpecId::OSAKA if Skadi is active
    /// ```
    pub(crate) fn for_mantle(&self, input: MantleEvmEnvInput) -> EvmEnv<OpSpecId> {
        // Use Mantle-aware trait method to determine spec
        // This automatically handles both Mantle-specific and OP Stack hardforks
        let spec = self.chain_spec().revm_spec_at_timestamp(input.timestamp);

        debug!(spec = ?spec, timestamp = input.timestamp, "Computed Mantle EVM spec");

        let cfg_env = CfgEnv::new().with_chain_id(self.chain_spec().chain().id()).with_spec(spec);

        // Calculate blob excess gas and price for EIP-4844 (Cancun+)
        let blob_excess_gas_and_price =
            spec.into_eth_spec().is_enabled_in(SpecId::CANCUN).then(|| {
                let excess_blob_gas = input.excess_blob_gas.unwrap_or(0);
                let blob_gasprice = alloy_eips::eip4844::calc_blob_gasprice(excess_blob_gas);
                BlobExcessGasAndPrice { excess_blob_gas, blob_gasprice }
            });

        let is_merge_active = spec.into_eth_spec() >= SpecId::MERGE;

        let block_env = BlockEnv {
            number: U256::from(input.number),
            beneficiary: input.beneficiary,
            timestamp: U256::from(input.timestamp),
            difficulty: if is_merge_active { U256::ZERO } else { input.difficulty },
            prevrandao: if is_merge_active { input.mix_hash } else { None },
            gas_limit: input.gas_limit,
            basefee: input.base_fee_per_gas,
            blob_excess_gas_and_price,
        };

        EvmEnv { cfg_env, block_env }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_primitives::{address, b256};
    use reth_optimism_chainspec::{OpChainSpec, MANTLE_MAINNET};

    #[test]
    fn test_mantle_evm_env_input_from_block_header() {
        let header = Header {
            number: 1000,
            timestamp: 1700000000,
            beneficiary: address!("0000000000000000000000000000000000000001"),
            gas_limit: 30_000_000,
            base_fee_per_gas: Some(1000),
            difficulty: U256::from(12345),
            excess_blob_gas: Some(131072),
            ..Default::default()
        };

        let input = MantleEvmEnvInput::from_block_header(&header);

        assert_eq!(input.timestamp, 1700000000);
        assert_eq!(input.number, 1000);
        assert_eq!(input.beneficiary, address!("0000000000000000000000000000000000000001"));
        assert_eq!(input.gas_limit, 30_000_000);
        assert_eq!(input.base_fee_per_gas, 1000);
        assert_eq!(input.difficulty, U256::from(12345));
        assert_eq!(input.mix_hash, header.mix_hash());
        assert_eq!(input.excess_blob_gas, Some(131072));
    }

    #[test]
    fn test_mantle_evm_env_input_for_next() {
        let parent = Header {
            number: 999,
            timestamp: 1699999998,
            beneficiary: address!("0000000000000000000000000000000000000001"),
            gas_limit: 30_000_000,
            base_fee_per_gas: Some(900),
            ..Default::default()
        };

        let attributes = OpNextBlockEnvAttributes {
            timestamp: 1700000000,
            suggested_fee_recipient: address!("0000000000000000000000000000000000000002"),
            prev_randao: b256!("2222222222222222222222222222222222222222222222222222222222222222"),
            gas_limit: 30_000_000,
            parent_beacon_block_root: None,
            extra_data: Default::default(),
        };

        let base_fee = 1000u64;
        let input = MantleEvmEnvInput::for_next(&parent, &attributes, base_fee);

        assert_eq!(input.timestamp, 1700000000);
        assert_eq!(input.number, 1000); // parent.number + 1
        assert_eq!(input.beneficiary, address!("0000000000000000000000000000000000000002"));
        assert_eq!(input.gas_limit, 30_000_000);
        assert_eq!(input.base_fee_per_gas, 1000);
        assert_eq!(input.difficulty, U256::ZERO); // Always zero for OP Stack chains
        assert_eq!(
            input.mix_hash,
            Some(b256!("2222222222222222222222222222222222222222222222222222222222222222"))
        );
        assert_eq!(input.excess_blob_gas, None);
    }

    #[test]
    fn test_for_mantle_uses_mantle_hardfork_trait() {
        use reth_optimism_primitives::OpPrimitives;

        // Create a config with Mantle mainnet spec
        let config = OpEvmConfig::<OpChainSpec, OpPrimitives>::optimism(MANTLE_MAINNET.clone());

        let header = Header {
            number: 1000,
            timestamp: 1700000000, // Before Skadi
            beneficiary: address!("0000000000000000000000000000000000000001"),
            gas_limit: 30_000_000,
            base_fee_per_gas: Some(1000),
            ..Default::default()
        };

        let input = MantleEvmEnvInput::from_block_header(&header);
        let evm_env = config.for_mantle(input);

        // Verify that the spec is determined correctly
        assert_eq!(evm_env.cfg_env.chain_id, MANTLE_MAINNET.chain().id());

        // The spec should be determined by the timestamp
        // This test verifies that for_mantle is being called correctly
        assert!(evm_env.cfg_env.spec != OpSpecId::OSAKA); // Not Skadi yet at this timestamp
    }

    #[test]
    fn test_for_mantle_skadi_hardfork_detection() {
        use reth_optimism_primitives::OpPrimitives;

        // Create config
        let config = OpEvmConfig::<OpChainSpec, OpPrimitives>::optimism(MANTLE_MAINNET.clone());

        // Test before Skadi activation
        let header_before = Header {
            number: 1000,
            timestamp: 1600000000, // Before Skadi
            beneficiary: address!("0000000000000000000000000000000000000001"),
            gas_limit: 30_000_000,
            base_fee_per_gas: Some(1000),
            ..Default::default()
        };

        let input_before = MantleEvmEnvInput::from_block_header(&header_before);
        let evm_env_before = config.for_mantle(input_before);

        // Should not be OSAKA before Skadi
        let spec_before = MANTLE_MAINNET.revm_spec_at_timestamp(1600000000);
        assert_eq!(evm_env_before.cfg_env.spec, spec_before);

        // Test after Skadi activation (if configured in MANTLE_MAINNET)
        // Note: This test demonstrates the pattern; actual activation time depends on chain config
        let timestamp_future = 2000000000u64;
        if MANTLE_MAINNET.is_skadi_active_at_timestamp(timestamp_future) {
            let header_after = Header {
                number: 2000,
                timestamp: timestamp_future,
                beneficiary: address!("0000000000000000000000000000000000000001"),
                gas_limit: 30_000_000,
                base_fee_per_gas: Some(1000),
                ..Default::default()
            };

            let input_after = MantleEvmEnvInput::from_block_header(&header_after);
            let evm_env_after = config.for_mantle(input_after);

            // Should be OSAKA when Skadi is active
            assert_eq!(evm_env_after.cfg_env.spec, OpSpecId::OSAKA);
        }
    }

    #[test]
    fn test_for_mantle_blob_gas_calculation() {
        use reth_optimism_primitives::OpPrimitives;

        let config = OpEvmConfig::<OpChainSpec, OpPrimitives>::optimism(MANTLE_MAINNET.clone());

        // Create header with blob gas (post-Cancun scenario)
        let excess_blob_gas = 131072u64;
        let header = Header {
            number: 1000,
            timestamp: 1700000000,
            beneficiary: address!("0000000000000000000000000000000000000001"),
            gas_limit: 30_000_000,
            base_fee_per_gas: Some(1000),
            excess_blob_gas: Some(excess_blob_gas),
            ..Default::default()
        };

        let input = MantleEvmEnvInput::from_block_header(&header);
        let evm_env = config.for_mantle(input);

        // If Cancun is active, blob_excess_gas_and_price should be set
        let spec = MANTLE_MAINNET.revm_spec_at_timestamp(1700000000);
        if spec.into_eth_spec().is_enabled_in(SpecId::CANCUN) {
            assert!(evm_env.block_env.blob_excess_gas_and_price.is_some());
            let blob_data = evm_env.block_env.blob_excess_gas_and_price.unwrap();
            assert_eq!(blob_data.excess_blob_gas, excess_blob_gas);

            // Verify blob gas price calculation
            let expected_price = alloy_eips::eip4844::calc_blob_gasprice(excess_blob_gas);
            assert_eq!(blob_data.blob_gasprice, expected_price);
        }
    }

    #[test]
    fn test_for_mantle_merge_behavior() {
        use reth_optimism_primitives::OpPrimitives;

        let config = OpEvmConfig::<OpChainSpec, OpPrimitives>::optimism(MANTLE_MAINNET.clone());

        let difficulty = U256::from(12345);

        let header = Header {
            number: 1000,
            timestamp: 1700000000,
            beneficiary: address!("0000000000000000000000000000000000000001"),
            gas_limit: 30_000_000,
            base_fee_per_gas: Some(1000),
            difficulty,
            extra_data: Default::default(),
            ..Default::default()
        };

        let input = MantleEvmEnvInput::from_block_header(&header);
        let evm_env = config.for_mantle(input);

        let spec = MANTLE_MAINNET.revm_spec_at_timestamp(1700000000);
        let is_merge_active = spec.into_eth_spec() >= SpecId::MERGE;

        // All OP Stack chains (including Mantle) are post-merge
        // So we expect merge behavior
        assert!(is_merge_active);
        assert_eq!(evm_env.block_env.difficulty, U256::ZERO);

        // prevrandao will be set from header's mix_hash
        // For OP Stack, this is typically derived from the beacon chain
    }

    #[test]
    fn test_mantle_evm_env_input_constructors_consistency() {
        // Verify that all three constructors produce valid inputs

        // 1. from_block_header
        let header = Header {
            number: 1000,
            timestamp: 1700000000,
            beneficiary: address!("0000000000000000000000000000000000000001"),
            gas_limit: 30_000_000,
            base_fee_per_gas: Some(1000),
            ..Default::default()
        };
        let input1 = MantleEvmEnvInput::from_block_header(&header);
        assert_eq!(input1.number, 1000);
        assert_eq!(input1.timestamp, 1700000000);

        // 2. for_next
        let prev_randao = b256!("4444444444444444444444444444444444444444444444444444444444444444");
        let attributes = OpNextBlockEnvAttributes {
            timestamp: 1700000002,
            suggested_fee_recipient: address!("0000000000000000000000000000000000000002"),
            prev_randao,
            gas_limit: 30_000_000,
            parent_beacon_block_root: None,
            extra_data: Default::default(),
        };
        let input2 = MantleEvmEnvInput::for_next(&header, &attributes, 1100);
        assert_eq!(input2.number, 1001); // parent.number + 1
        assert_eq!(input2.timestamp, 1700000002);
        assert_eq!(input2.base_fee_per_gas, 1100);
        assert_eq!(input2.difficulty, U256::ZERO); // Always zero for next block
        assert_eq!(input2.mix_hash, Some(prev_randao));
    }
}

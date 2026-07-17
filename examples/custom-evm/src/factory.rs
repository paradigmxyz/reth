//! Factory implementations used by the custom EVM example.

use crate::{
    config::{custom_version, CustomConfigSelector, CustomTypes},
    opcode,
    tx::{custom_registry, CustomEnvelope},
};
use alloy_primitives::{address, Address, Bytes};
use evm2::{
    env::BlockEnv,
    evm::precompile::PrecompileOutput,
    interpreter::{GasTracker, Message},
    precompiles::{Precompile, PrecompileId, PrecompileResult},
    BaseEvmTypes, Evm, EvmConfig, EvmConfigSelector, EvmTypesHost, ExecutionConfig, OpcodeConfig,
    SpecId, Version,
};
use reth_ethereum::evm::EvmFactory;

/// Address of the example custom precompile.
pub const CUSTOM_PRECOMPILE_ADDRESS: Address =
    address!("0x0000000000000000000000000000000000000999");

/// A precompile that returns a fixed 32-byte value.
pub fn custom_precompile<T: EvmTypesHost>(
    _evm: &mut Evm<'_, T>,
    _message: &Message<T>,
    _gas: &mut GasTracker,
) -> PrecompileResult {
    Ok(PrecompileOutput::new(Bytes::from_static(&[0x42; 32])))
}

/// Returns the base precompiles plus the example precompile.
pub fn custom_precompiles<T: EvmTypesHost>(spec: SpecId) -> evm2::Precompiles<T> {
    let mut precompiles = evm2::Precompiles::base(spec);
    precompiles.as_map_mut().insert(Precompile::new(
        CUSTOM_PRECOMPILE_ADDRESS,
        PrecompileId::custom("custom-fixed-value"),
        custom_precompile::<T>,
    ));
    precompiles
}

/// Standard Ethereum type-family configuration used by the node example.
#[derive(Clone, Copy, Debug)]
pub struct NodeConfig;

impl EvmConfig<BaseEvmTypes> for NodeConfig {
    const BASE_SPEC_ID: SpecId = SpecId::OSAKA;
    const OPCODE_CONFIG: &'static OpcodeConfig<BaseEvmTypes> = &node_opcode_config();
}

/// Factory used by the standard Ethereum node path.
#[derive(Clone, Copy, Debug, Default)]
pub struct NodeEvmFactory;

impl EvmFactory for NodeEvmFactory {
    type Types = BaseEvmTypes;
    type SpecId = SpecId;

    fn spec_id(&self, spec: SpecId) -> SpecId {
        spec
    }

    fn version(&self, spec: SpecId, chain_id: u64) -> Version {
        let mut version = custom_version(spec);
        version.chain_id = chain_id;
        version
    }

    fn execution_config(
        &self,
        _spec: Self::SpecId,
        version: Version,
    ) -> ExecutionConfig<Self::Types> {
        ExecutionConfig::for_config::<NodeConfig>().with_version(version)
    }

    fn tx_registry(
        &self,
        spec: Self::SpecId,
    ) -> evm2::registry::TxRegistry<Self::Types, evm2::TxResult<Self::Types>> {
        evm2::ethereum::ethereum_tx_registry(spec)
    }

    fn precompiles(&self, spec: Self::SpecId) -> evm2::Precompiles<Self::Types> {
        custom_precompiles(spec)
    }
}

/// Factory used by the custom-envelope path.
#[derive(Clone, Copy, Debug, Default)]
pub struct CustomEvmFactory;

impl EvmFactory<CustomEnvelope> for CustomEvmFactory {
    type Types = CustomTypes;
    type SpecId = crate::config::CustomSpecId;

    fn spec_id(&self, spec: SpecId) -> Self::SpecId {
        if spec.enables(SpecId::OSAKA) {
            Self::SpecId::CustomOsaka
        } else {
            Self::SpecId::MainnetOsaka
        }
    }

    fn version(&self, spec: SpecId, chain_id: u64) -> Version {
        let mut version = custom_version(spec);
        version.chain_id = chain_id;
        version
    }

    fn execution_config(
        &self,
        spec: Self::SpecId,
        version: Version,
    ) -> ExecutionConfig<Self::Types> {
        CustomConfigSelector::execution_config(spec).with_version(version)
    }

    fn tx_registry(
        &self,
        _spec: Self::SpecId,
    ) -> evm2::registry::TxRegistry<Self::Types, evm2::TxResult<Self::Types>> {
        custom_registry()
    }

    fn precompiles(&self, spec: Self::SpecId) -> evm2::Precompiles<Self::Types> {
        custom_precompiles(spec.into())
    }
}

/// Creates the custom opcode table for the standard Ethereum type family.
pub const fn node_opcode_config() -> OpcodeConfig<BaseEvmTypes> {
    let mut config = OpcodeConfig::<BaseEvmTypes>::base::<NodeConfig>();
    config.set_instruction::<opcode::custom<BaseEvmTypes>>(
        opcode::CUSTOM_OPCODE,
        opcode::CUSTOM_OPCODE_GAS,
    );
    config
}

/// Returns a default block environment with the custom type family.
pub fn custom_block_env() -> BlockEnv<CustomTypes> {
    BlockEnv::default()
}

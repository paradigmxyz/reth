//! Custom evm2 type families and version selectors.

use crate::{opcode, tx::CustomEnvelope};
use evm2::{
    BaseEvmConfig, Evm, EvmConfig, EvmConfigSelector, EvmTypesHost, ExecutionConfig, OpcodeConfig,
    SpecId, Version,
};

/// Runtime specification IDs used by the custom evm2 example.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CustomSpecId {
    /// Unmodified Osaka execution.
    MainnetOsaka,
    /// Osaka execution with the example extensions enabled.
    CustomOsaka,
}

impl CustomSpecId {
    /// First runtime specification ID.
    pub const MIN: Self = Self::MainnetOsaka;
    /// Last runtime specification ID.
    pub const NEXT: Self = Self::CustomOsaka;

    /// Returns the numeric representation used by evm2's const selectors.
    pub const fn as_u32(self) -> u32 {
        self as u32
    }

    /// Converts a numeric runtime specification ID.
    pub const fn try_from_u32(spec_id: u32) -> Option<Self> {
        if spec_id <= Self::NEXT as u32 {
            // SAFETY: both enum variants are contiguous and start at zero.
            return Some(unsafe { core::mem::transmute::<u32, Self>(spec_id) })
        }
        None
    }

    /// Returns whether this specification includes `other`.
    pub const fn enables(self, other: Self) -> bool {
        self as u32 >= other as u32
    }
}

impl From<CustomSpecId> for u32 {
    fn from(spec_id: CustomSpecId) -> Self {
        spec_id as Self
    }
}

impl From<CustomSpecId> for SpecId {
    fn from(spec_id: CustomSpecId) -> Self {
        match spec_id {
            CustomSpecId::MainnetOsaka | CustomSpecId::CustomOsaka => Self::OSAKA,
        }
    }
}

/// evm2 type family used by the custom-envelope execution path.
#[derive(Clone, Copy, Debug)]
pub struct CustomTypes;

impl EvmTypesHost for CustomTypes {
    type ConfigSelector = CustomConfigSelector;
    type SpecId = CustomSpecId;
    type Tx = CustomEnvelope;
    type EvmExt = ();
    type MessageExt = CustomMessageExt;
    type MessageResultExt = CustomMessageResultExt;
    type TxEnvExt = CustomTxEnvExt;
    type TxResultExt = CustomTxResultExt;
    type BlockEnvExt = CustomBlockEnvExt;
    type Host<'a> = Evm<'a, Self>;
}

/// Extra message state consumed by custom handlers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CustomMessageExt {
    /// Whether the message originated from a system transaction.
    pub is_system: bool,
}

/// Extra result state emitted by custom handlers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CustomMessageResultExt {
    /// Whether the handler consumed a custom message.
    pub handled_custom_message: bool,
}

/// Extra transaction-environment state consumed by custom handlers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CustomTxEnvExt {
    /// Human-readable transaction label.
    pub label: &'static str,
}

/// Extra transaction-result state emitted by custom handlers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CustomTxResultExt {
    /// Whether the handler consumed a custom transaction.
    pub handled_custom_tx: bool,
}

/// Extra block state exposed to custom opcodes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CustomBlockEnvExt {
    /// L1 block number exposed to the example opcode.
    pub l1_block_number: u64,
}

/// Compile-time opcode table for one base/custom specification pair.
#[derive(Clone, Copy, Debug)]
pub struct CustomConfig<const BASE_SPEC_ID: u32, const CUSTOM_SPEC_ID: u32>;

impl<const BASE_SPEC_ID: u32, const CUSTOM_SPEC_ID: u32> EvmConfig<CustomTypes>
    for CustomConfig<BASE_SPEC_ID, CUSTOM_SPEC_ID>
{
    const BASE_SPEC_ID: SpecId = SpecId::try_from_u32(BASE_SPEC_ID).unwrap();
    const OPCODE_CONFIG: &'static OpcodeConfig<CustomTypes> =
        &custom_opcode_config::<BASE_SPEC_ID, CUSTOM_SPEC_ID>();
}

/// Builds the custom runtime version from an inherited base specification.
pub fn custom_version(base_spec_id: SpecId) -> Version {
    let mut version = *Version::base(base_spec_id);
    opcode::install_gas_params(&mut version.gas_params);
    version
}

/// Builds the custom opcode table for the selected runtime specification.
pub const fn custom_opcode_config<const BASE_SPEC_ID: u32, const CUSTOM_SPEC_ID: u32>(
) -> OpcodeConfig<CustomTypes> {
    let mut config =
        OpcodeConfig::<CustomTypes>::base::<CustomConfig<BASE_SPEC_ID, CUSTOM_SPEC_ID>>();
    let custom_spec_id =
        CustomSpecId::try_from_u32(CUSTOM_SPEC_ID).expect("invalid custom spec id");
    if custom_spec_id.enables(CustomSpecId::CustomOsaka) {
        config.set_instruction::<opcode::custom<CustomTypes>>(
            opcode::CUSTOM_OPCODE,
            opcode::CUSTOM_OPCODE_GAS,
        );
        config.set_instruction::<opcode::l1_blocknumber>(
            opcode::L1_BLOCKNUMBER_OPCODE,
            opcode::L1_BLOCKNUMBER_GAS,
        );
    }
    config
}

/// Runtime selector for the custom type family.
#[derive(Clone, Copy, Debug)]
pub struct CustomConfigSelector;

impl EvmConfigSelector<CustomTypes> for CustomConfigSelector {
    type Config<const BASE_SPEC_ID: u32, const CUSTOM_SPEC_ID: u32> =
        CustomConfig<BASE_SPEC_ID, CUSTOM_SPEC_ID>;

    fn execution_config(spec_id: CustomSpecId) -> ExecutionConfig<CustomTypes> {
        match spec_id {
            CustomSpecId::MainnetOsaka => {
                ExecutionConfig::for_config::<BaseEvmConfig<{ SpecId::OSAKA as u32 }>>()
            }
            CustomSpecId::CustomOsaka => ExecutionConfig::for_config::<
                CustomConfig<{ SpecId::OSAKA as u32 }, { CustomSpecId::CustomOsaka as u32 }>,
            >(),
        }
    }
}

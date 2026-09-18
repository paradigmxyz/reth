//! Custom evm2 opcode definitions.

use crate::config::CustomTypes;
use evm2::{
    interpreter::{Host, Word},
    version::{GasId, GasParams},
};
use evm2_macros::instruction;

/// Opcode used by the example transaction handler.
pub const CUSTOM_OPCODE: u8 = 0x0c;
/// Static gas charged by [`custom`].
pub const CUSTOM_OPCODE_GAS: u16 = 7;
/// Dynamic gas parameter used by [`custom`].
pub const CUSTOM_OPCODE_DYNAMIC_GAS_ID: GasId = GasId::Custom0;
/// Dynamic gas charged by [`custom`].
pub const CUSTOM_OPCODE_DYNAMIC_GAS: u32 = 3;
/// Opcode that reads [`crate::config::CustomBlockEnvExt`].
pub const L1_BLOCKNUMBER_OPCODE: u8 = 0x0d;
/// Static gas charged by [`l1_blocknumber`].
pub const L1_BLOCKNUMBER_GAS: u16 = 2;

/// Installs the dynamic gas parameter used by the custom opcode.
pub const fn install_gas_params(gas_params: &mut GasParams) {
    gas_params.set(CUSTOM_OPCODE_DYNAMIC_GAS_ID, CUSTOM_OPCODE_DYNAMIC_GAS);
}

/// Example opcode that pushes `0xdead` onto the stack.
#[instruction(dynamic_gas)]
pub fn custom(cx: _) -> Result<out> {
    cx.gas.spend(cx.state.gas_params().get(CUSTOM_OPCODE_DYNAMIC_GAS_ID).into())?;
    *out = Word::from(0xdead_u64);
}

/// Example opcode that reads the L1 block number from the block extension.
#[instruction(EvmTypes = CustomTypes)]
pub fn l1_blocknumber(cx: _) -> out {
    *out = Word::from(cx.state.host().block_env().ext.l1_block_number);
}

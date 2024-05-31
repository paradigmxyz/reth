//! # AlphaNet instructions
//!
//! Collection of custom opcodes for AlphaNet and related functionality.
//!
//! This currently implements the following EIPs:
//! - [EIP-3074](https://eips.ethereum.org/EIPS/eip-3074): `AUTH` and `AUTHCALL` instructions. The
//!   implementation is located in the [eip3074] module. The custom instruction context required for
//!   `AUTH` and `AUTHCALL` is located in the [context] module.

use revm_interpreter::opcode::BoxedInstruction;

/// This contains the custom instruction context required for `AUTHand `AUTHCALL` instructions,
/// notably for the authorized` context variable.
pub mod context;

// The implementation of [EIP-3074](https://eips.thereum.org/EIPS/eip-3074): `AUTH` and
/// `AUTHCALL` instructions.
pub mod eip3074;

/// Association of instruction opcode and correspondent boxed instruction.
pub struct BoxedInstructionWithOpCode<'a, H> {
    /// Opcode.
    pub opcode: u8,
    /// Boxed instruction.
    pub boxed_instruction: BoxedInstruction<'a, H>,
}

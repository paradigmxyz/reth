//! Opcount tracing inspector that simply counts all opcodes.
//!
//! See also <https://geth.ethereum.org/docs/developers/evm-tracing/built-in-tracers>

use revm::{
    interpreter::{InstructionResult, Interpreter},
    Database, EVMData, Inspector,
};

/// An inspector that counts all opcodes.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpcodeCountInspector {
    /// opcode counter
    count: usize,
}

impl OpcodeCountInspector {
    /// Returns the opcode counter
    pub fn count(&self) -> usize {
        self.count
    }
}

impl<DB> Inspector<DB> for OpcodeCountInspector
where
    DB: Database,
{
    fn step(
        &mut self,
        _interp: &mut Interpreter,
        _data: &mut EVMData<'_, DB>,
    ) -> InstructionResult {
        self.count += 1;
        InstructionResult::Continue
    }
}

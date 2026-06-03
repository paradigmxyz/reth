//! EVM interpreter types.

pub use revm::{
    bytecode::opcode::OpCode,
    interpreter::{interpreter::EthInterpreter, interpreter_types::Jumps, Interpreter},
};

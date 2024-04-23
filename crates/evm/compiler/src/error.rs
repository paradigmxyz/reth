use reth_primitives::{fs, B256};
use std::io;
use thiserror::Error;

/// EVM compiler error.
#[derive(Debug, Error)]
pub enum EvmCompilerError {
    /// Empty bytecode.
    #[error("missing or empty bytecode for contract {_0}")]
    EmptyBytecode(usize),
    /// File system error.
    #[error(transparent)]
    Fs(#[from] fs::FsPathError),
    /// I/O error.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// JSON error.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Confy error.
    #[error(transparent)]
    Confy(#[from] confy::ConfyError),
    /// Hex error.
    #[error(transparent)]
    Hex(#[from] reth_primitives::hex::FromHexError),
    /// Compilation error.
    #[error("failed to compile contract with hash {_0}: {_1}")]
    Compile(B256, #[source] revm_jit::Error),
}

/// EVM compiler result.
pub type EvmCompilerResult<T> = Result<T, EvmCompilerError>;

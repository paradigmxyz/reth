use reth_fs_util as fs;
use reth_primitives::B256;
use revm::primitives::SpecId;
use std::io;
use thiserror::Error;

/// EVM compiler result.
pub type EvmCompilerResult<T> = Result<T, EvmCompilerError>;

/// EVM compiler error.
#[derive(Debug, Error)]
pub enum EvmCompilerError {
    /// Empty bytecode.
    #[error("TOML contract {0}: missing or empty bytecode")]
    EmptyBytecode(usize),
    /// Invalid EVM version.
    #[error("TOML contract {0}: {1:?} is not supported as an EVM version")]
    InvalidEvmVersion(usize, SpecId),
    /// Invalid EVM version range.
    #[error("TOML contract {0}: starting EVM version is greater than ending EVM version")]
    InvalidEvmVersionRange(usize),
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
    #[error("failed to compile contract with hash {0}: {1}")]
    Compile(B256, #[source] revm_jit::Error),
}

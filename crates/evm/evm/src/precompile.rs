//! EVM precompile types.

pub use revm::{
    handler::{EthPrecompiles, PrecompileProvider},
    precompile::{
        Precompile, PrecompileId, PrecompileOutput, PrecompileResult, PrecompileStatus, Precompiles,
    },
};

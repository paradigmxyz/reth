//! Scroll `revm` types redefinitions. Account types are redefined with two additional fields
//! `code_size` and `poseidon_code_hash`, which are used during computation of the state root.

#![warn(unused_crate_dependencies)]
#![cfg_attr(not(feature = "std"), no_std)]

pub use revm::primitives::AccountInfo;

#[cfg(all(feature = "optimism", not(feature = "scroll")))]
pub use revm::{primitives::OptimismFields, L1BlockInfo, L1_BLOCK_CONTRACT};

pub use revm::{
    db::*,
    inspector_handle_register,
    primitives::{
        keccak256, AuthorizationList, Bytecode, BytecodeDecodeError, JumpTable,
        LegacyAnalyzedBytecode, TxEnv, TxKind,
    },
    ContextPrecompile, ContextPrecompiles, Evm, EvmBuilder, EvmContext, GetInspector, Inspector,
    JournaledState,
};

/// Match the `revm` module structure
pub mod handler {
    pub use revm::handler::*;
}

/// Match the `revm` module structure
pub mod interpreter {
    pub use revm::interpreter::*;
}

/// Match the `revm` module structure
pub mod inspectors {
    pub use revm::inspectors::*;
}

/// Match the `revm` module structure
pub mod precompile {
    pub use revm::precompile::*;
}

/// Match the `revm-primitives` module structure
pub mod primitives {
    pub use revm::primitives::*;
}

/// Match the `revm` module structure
pub mod db {
    pub use revm::db::*;
    /// Match the `revm` module structure
    pub mod states {
        pub use revm::db::states::*;
    }
}

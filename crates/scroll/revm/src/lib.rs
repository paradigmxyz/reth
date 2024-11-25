//! Scroll `revm` types redefinitions. Account types are redefined with two additional fields
//! `code_size` and `poseidon_code_hash`, which are used during computation of the state root.

#![warn(unused_crate_dependencies)]

pub mod states;

#[cfg(feature = "optimism")]
pub use revm::primitives::OptimismFields;

pub use revm::{
    db::*,
    inspector_handle_register,
    primitives::{
        keccak256, AuthorizationList, Bytecode, BytecodeDecodeError, JumpTable,
        LegacyAnalyzedBytecode, TxEnv, TxKind,
    },
    Evm, EvmBuilder, GetInspector,
};

#[cfg(feature = "scroll")]
pub use crate::states::ScrollAccountInfo as AccountInfo;
#[cfg(not(feature = "scroll"))]
pub use revm::primitives::AccountInfo;
pub use states::ScrollAccountInfo;

/// Shared module, available for all feature flags.
pub mod shared {
    pub use revm::primitives::AccountInfo;
}

/// Match the `revm` module structure
pub mod interpreter {
    pub use revm::interpreter::*;
}

/// Match the `revm` module structure
pub mod precompile {
    pub use revm::precompile::*;
}

/// Match the `revm-primitives` module structure
pub mod primitives {
    #[cfg(feature = "scroll")]
    pub use crate::states::ScrollAccountInfo as AccountInfo;
    pub use revm::primitives::*;
}

/// Match the `revm` module structure
pub mod db {
    #[cfg(feature = "scroll")]
    pub use crate::states::{
        ScrollBundleAccount as BundleAccount, ScrollBundleState as BundleState,
    };
    pub use revm::db::*;
    /// Match the `revm` module structure
    pub mod states {
        #[cfg(feature = "scroll")]
        pub use crate::states::{
            ScrollBundleBuilder as BundleBuilder, ScrollBundleState as BundleState,
            ScrollPlainStateReverts as PlainStateReverts, ScrollStateChangeset as StateChangeset,
        };
        pub use revm::db::states::*;
    }
}

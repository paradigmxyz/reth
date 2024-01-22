//! The core of the Ethereum node. Collection of utilities and libraries that are used by the node.

#![allow(missing_docs)]
#![allow(missing_debug_implementations)]
#![allow(dead_code)]

pub mod args;
pub mod cli;
pub mod dirs;
pub mod init;
pub mod utils;
pub mod version;

/// Re-exported from `reth_primitives`.
pub mod primitives {
    pub use reth_primitives::*;
}

/// Re-export of `reth_rpc_*` crates.
pub mod rpc {

    /// Re-exported from `reth_rpc_builder`.
    pub mod builder {
        pub use reth_rpc_builder::*;
    }

    /// Re-exported from `reth_rpc_types`.
    pub mod types {
        pub use reth_rpc_types::*;
    }

    /// Re-exported from `reth_rpc_api`.
    pub mod api {
        pub use reth_rpc_api::*;
    }
    /// Re-exported from `reth_rpc::eth`.
    pub mod eth {
        pub use reth_rpc::eth::*;
    }

    /// Re-exported from `reth_rpc::rpc`.
    pub mod result {
        pub use reth_rpc::result::*;
    }

    /// Re-exported from `reth_rpc::eth`.
    pub mod compat {
        pub use reth_rpc_types_compat::*;
    }
}

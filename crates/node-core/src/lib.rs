//! The core of the Ethereum node. Collection of utilities and libraries that are used by the node.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![warn(unused_crate_dependencies)]

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

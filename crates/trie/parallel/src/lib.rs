//! Parallel proof computation, trie verification, and state-root task interface types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

/// Error types for the state-root task and proof computation.
pub mod error;

/// Implementation of parallel proof computation.
pub mod proof_task;

/// State root task interface types shared between the engine tree and the payload builder.
pub mod state_root_task;

pub mod verify;

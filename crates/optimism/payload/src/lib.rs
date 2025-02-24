//! Optimism's payload builder implementation.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![allow(clippy::useless_let_if_seq)]

extern crate alloc;

pub mod builder;
pub use builder::OpPayloadBuilder;
pub mod error;
pub mod payload;
pub use payload::{OpBuiltPayload, OpPayloadAttributes, OpPayloadBuilderAttributes};
mod traits;
pub use traits::*;
pub mod validator;
pub use validator::OpExecutionPayloadValidator;

pub mod config;

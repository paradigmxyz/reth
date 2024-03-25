//! Standalone crate for Reth configuration traits and builder types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

/// Traits, validation methods, and helper types used to abstract over engine types.
///
/// Notably contains the [EngineTypes] trait and implementations for ethereum mainnet types.
pub mod engine;
pub use engine::{
    validate_payload_timestamp, validate_version_specific_fields, validate_withdrawals_presence,
    BuiltPayload, EngineApiMessageVersion, EngineObjectValidationError, EngineTypes,
    MessageValidationKind, PayloadAttributes, PayloadBuilderAttributes, PayloadOrAttributes,
    VersionSpecificValidationError,
};

/// Traits and helper types used to abstract over EVM methods and types.
pub mod evm;
pub use evm::{ConfigureEvm, ConfigureEvmEnv};

pub mod primitives;

pub mod node;
pub use node::NodeTypes;

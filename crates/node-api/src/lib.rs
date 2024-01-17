//! Standalone crate for Reth configuration traits and builder types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

/// Traits, validation methods, and helper types used to abstract over engine types.
///
/// Notably contains the [EngineTypes] trait and implementations for ethereum mainnet types.
pub mod engine;
#[cfg(feature = "optimism")]
pub use engine::optimism_validate_version_specific_fields;
pub use engine::{
    validate_payload_timestamp, validate_version_specific_fields, validate_withdrawals_presence,
    AttributesValidationError, BuiltPayload, EngineApiMessageVersion, EngineTypes,
    PayloadAttributes, PayloadBuilderAttributes, PayloadOrAttributes,
};

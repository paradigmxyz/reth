//! Standalone crate for Reth configuration traits and builder types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

/// Contains the [EngineTypes] trait, used to configure the types used by the engine. Also contains
/// methods that can be used to validate payload attributes.
pub mod engine;
pub use engine::{
    validate_payload_timestamp, validate_version_specific_fields, validate_withdrawals_presence,
    EngineApiMessageVersion, EngineTypes,
};

/// Contains traits and types used to abstract over payload attributes types.
pub mod payload_attributes;
pub use payload_attributes::{PayloadAttributesTrait, PayloadBuilderAttributesTrait};

/// Contains error types used in the traits defined in this crate.
pub mod error;
pub use error::AttributesValidationError;

/// Contains types used in implementations of [PayloadAttributesTrait].
pub mod payload;
pub use payload::PayloadOrAttributes;

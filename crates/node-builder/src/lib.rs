//! Standalone crate for Reth configuration traits and builder types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

/// Contains the [EngineTypes] trait, used to configure the types used by the engine.
pub mod engine_types;
pub use engine_types::EngineTypes;

/// Contains traits and types used to abstract over payload attributes types.
pub mod payload_attributes;
pub use payload_attributes::{PayloadAttributesTrait, PayloadBuilderAttributesTrait};

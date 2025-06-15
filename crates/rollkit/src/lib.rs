//! Rollkit payload builder for Reth integration.
//!
//! This crate provides a complete rollkit integration for Reth, including:
//! - Custom payload builder that supports transactions via Engine API
//! - Rollkit-specific node types and configurations
//! - Engine API validation and processing for rollkit blocks

mod builder;
mod config;
mod types;

#[cfg(test)]
mod tests;

// Re-export all public types and functions
pub use builder::{create_payload_builder_service, RollkitPayloadBuilder};
pub use config::{ConfigError, RollkitPayloadBuilderConfig};
pub use types::{PayloadAttributesError, RollkitPayloadAttributes}; 
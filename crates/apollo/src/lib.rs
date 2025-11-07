//! Apollo configuration client for dynamic configuration management.

/// Apollo client module
pub mod client;
/// Apollo namespace module
pub mod namespace;
/// Apollo types and configuration
pub mod types;

/// Retrieves configuration from Apollo Config Service with fallback to default value.
///
/// This macro attempts to fetch a configuration value from the Apollo Config Service.
/// If the value cannot be retrieved (Apollo client not initialized, key doesn't exist,
/// or type conversion fails), it returns the provided default value and logs a warning.
///
/// # Parameters
///
/// - `$namespace` - The Apollo namespace (e.g., "opreth_jsonrpc", "opreth_sequencer")
/// - `$key` - The configuration key to look up (e.g., "gpo.maxprice")
/// - `$default` - The fallback value if retrieval fails
#[macro_export]
macro_rules! apollo_config_or {
    ($namespace:expr, $key:expr, $default:expr) => {{
        let ns = $namespace;  // Bind to extend lifetime
        let ns_ref: &str = &ns;

        $crate::client::ApolloService::get_instance()
            .ok()
            .and_then(|apollo| apollo.try_get_cached_config(ns_ref, $key))
            .and_then(|v| $crate::types::FromConfigValue::try_from_config_value(&v))
            .unwrap_or_else( || {
                tracing::warn!(
                    target: "reth::apollo",
                    namespace = ns_ref,
                    key = $key,
                    default = ?$default,
                    "[Apollo] Using default config (client not initialized, key missing, or type mismatch)"
                );
                $default
            })
    }};
}

pub use client::ApolloService;
pub use types::ApolloConfig;

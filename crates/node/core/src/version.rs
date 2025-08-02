//! Version information for reth.
use std::sync::OnceLock;

use alloy_primitives::Bytes;
use alloy_rpc_types_engine::ClientCode;
use reth_db::ClientVersion;

/// The client code for Reth
pub const CLIENT_CODE: ClientCode = ClientCode::RH;

/// Constants for reth-cli
#[derive(Debug, Default)]
pub struct RethCliVersionConsts<'a> {
    /// The human readable name of the client
    pub name_client: &'a str,

    /// The latest version from Cargo.toml.
    pub cargo_pkg_version: &'a str,

    /// The full SHA of the latest commit.
    pub vergen_git_sha_long: &'a str,

    /// The 8 character short SHA of the latest commit.
    pub vergen_git_sha: &'a str,

    /// The build timestamp.
    pub vergen_build_timestamp: &'a str,

    /// The target triple.
    pub vergen_cargo_target_triple: &'a str,

    /// The build features.
    pub vergen_cargo_features: &'a str,

    /// The short version information for reth.
    pub short_version: &'a str,

    /// The long version information for reth.
    pub long_version: &'a str,
    /// The build profile name.
    pub build_profile_name: &'a str,

    /// The version information for reth formatted for P2P (devp2p).
    ///
    /// - The latest version from Cargo.toml
    /// - The target triple
    ///
    /// # Example
    ///
    /// ```text
    /// reth/v{major}.{minor}.{patch}-{sha1}/{target}
    /// ```
    /// e.g.: `reth/v0.1.0-alpha.1-428a6dc2f/aarch64-apple-darwin`
    pub p2p_client_version: &'a str,

    /// extra data used for payload building
    pub extra_data: String,
}

/// The default extra data used for payload building.
///
/// - The latest version from Cargo.toml
/// - The OS identifier
///
/// # Example
///
/// ```text
/// reth/v{major}.{minor}.{patch}/{OS}
/// ```
pub fn default_extra_data() -> String {
    format!("reth/v{}/{}", env!("CARGO_PKG_VERSION"), std::env::consts::OS)
}

/// The default extra data in bytes.
/// See [`default_extra_data`].
pub fn default_extra_data_bytes() -> Bytes {
    Bytes::from(default_extra_data().as_bytes().to_vec())
}

/// The default client version accessing the database.
pub fn default_client_version() -> ClientVersion {
    ClientVersion {
        version: env!("CARGO_PKG_VERSION").to_string(),
        git_sha: env!("VERGEN_GIT_SHA_SHORT").to_string(),
        build_timestamp: env!("VERGEN_BUILD_TIMESTAMP").to_string(),
    }
}

/// Global static version metadata
static VERSION_METADATA: OnceLock<RethCliVersionConsts<'static>> = OnceLock::new();

/// Initialize the global version metadata.
pub fn init_version_metadata(metadata: RethCliVersionConsts<'static>) {
    let _ = VERSION_METADATA.set(metadata);
}

/// Get a reference to the initialized version metadata.
///
/// # Panics
/// If init_version_metadata() hasn't been called.
pub fn version_metadata() -> &'static RethCliVersionConsts<'static> {
    VERSION_METADATA.get().expect("Version metadata not initialized")
}

/// default version metadata using compile-time env! macros.
pub fn default_version_metadata() -> RethCliVersionConsts<'static> {
    RethCliVersionConsts {
        name_client: "Reth",
        cargo_pkg_version: env!("CARGO_PKG_VERSION"),
        vergen_git_sha_long: env!("VERGEN_GIT_SHA"),
        vergen_git_sha: env!("VERGEN_GIT_SHA_SHORT"),
        vergen_build_timestamp: env!("VERGEN_BUILD_TIMESTAMP"),
        vergen_cargo_target_triple: env!("VERGEN_CARGO_TARGET_TRIPLE"),
        vergen_cargo_features: env!("VERGEN_CARGO_FEATURES"),
        short_version: env!("RETH_SHORT_VERSION"),
        long_version: concat!(
            env!("RETH_LONG_VERSION_0"),
            "\n",
            env!("RETH_LONG_VERSION_1"),
            "\n",
            env!("RETH_LONG_VERSION_2"),
            "\n",
            env!("RETH_LONG_VERSION_3"),
            "\n",
            env!("RETH_LONG_VERSION_4")
        ),
        build_profile_name: env!("RETH_BUILD_PROFILE"),
        p2p_client_version: env!("RETH_P2P_CLIENT_VERSION"),
        extra_data: default_extra_data(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assert_extra_data_less_32bytes() {
        let extra_data = default_extra_data();
        assert!(extra_data.len() <= 32, "extra data must be less than 32 bytes: {extra_data}")
    }
}

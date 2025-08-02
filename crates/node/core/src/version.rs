//! Version information for reth.
use std::{borrow::Cow, sync::OnceLock};

use alloy_primitives::Bytes;
use alloy_rpc_types_engine::ClientCode;
use reth_db::ClientVersion;

/// The client code for Reth
pub const CLIENT_CODE: ClientCode = ClientCode::RH;

/// Global static version metadata
static VERSION_METADATA: OnceLock<RethCliVersionConsts> = OnceLock::new();

/// Initialize the global version metadata.
pub fn try_init_version_metadata(
    metadata: RethCliVersionConsts,
) -> Result<(), RethCliVersionConsts> {
    VERSION_METADATA.set(metadata)
}

/// Constants for reth-cli
#[derive(Debug, Default)]
pub struct RethCliVersionConsts {
    /// The human readable name of the client
    pub name_client: Cow<'static, str>,

    /// The latest version from Cargo.toml.
    pub cargo_pkg_version: Cow<'static, str>,

    /// The full SHA of the latest commit.
    pub vergen_git_sha_long: Cow<'static, str>,

    /// The 8 character short SHA of the latest commit.
    pub vergen_git_sha: Cow<'static, str>,

    /// The build timestamp.
    pub vergen_build_timestamp: Cow<'static, str>,

    /// The target triple.
    pub vergen_cargo_target_triple: Cow<'static, str>,

    /// The build features.
    pub vergen_cargo_features: Cow<'static, str>,

    /// The short version information for reth.
    pub short_version: Cow<'static, str>,

    /// The long version information for reth.
    pub long_version: Cow<'static, str>,
    /// The build profile name.
    pub build_profile_name: Cow<'static, str>,

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
    pub p2p_client_version: Cow<'static, str>,

    /// extra data used for payload building
    pub extra_data: Cow<'static, str>,
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

/// Get a reference to the initialized version metadata.
///
/// # Panics
/// If `init_version_metadata()` hasn't been called.
pub fn version_metadata() -> &'static RethCliVersionConsts {
    VERSION_METADATA.get().expect("Version metadata not initialized")
}

/// Get a reference to the global version metadata
pub fn get_version_metadata() -> &'static RethCliVersionConsts {
    VERSION_METADATA.get_or_init(default_version_metadata)
}

/// default version metadata using compile-time env! macros.
pub fn default_version_metadata() -> RethCliVersionConsts {
    RethCliVersionConsts {
        name_client: Cow::Borrowed("Reth"),
        cargo_pkg_version: Cow::Owned(env!("CARGO_PKG_VERSION").to_string()),
        vergen_git_sha_long: Cow::Owned(env!("VERGEN_GIT_SHA").to_string()),
        vergen_git_sha: Cow::Owned(env!("VERGEN_GIT_SHA_SHORT").to_string()),
        vergen_build_timestamp: Cow::Owned(env!("VERGEN_BUILD_TIMESTAMP").to_string()),
        vergen_cargo_target_triple: Cow::Owned(env!("VERGEN_CARGO_TARGET_TRIPLE").to_string()),
        vergen_cargo_features: Cow::Owned(env!("VERGEN_CARGO_FEATURES").to_string()),
        short_version: Cow::Owned(env!("RETH_SHORT_VERSION").to_string()),
        long_version: Cow::Owned(format!(
            "{}\n{}\n{}\n{}\n{}",
            env!("RETH_LONG_VERSION_0"),
            env!("RETH_LONG_VERSION_1"),
            env!("RETH_LONG_VERSION_2"),
            env!("RETH_LONG_VERSION_3"),
            env!("RETH_LONG_VERSION_4"),
        )),

        build_profile_name: Cow::Owned(env!("RETH_BUILD_PROFILE").to_string()),
        p2p_client_version: Cow::Owned(env!("RETH_P2P_CLIENT_VERSION").to_string()),
        extra_data: Cow::Owned(default_extra_data()),
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

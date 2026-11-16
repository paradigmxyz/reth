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
///
/// Global defaults can be set via [`try_init_version_metadata`].
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
    let meta = version_metadata();
    ClientVersion {
        version: meta.cargo_pkg_version.to_string(),
        git_sha: meta.vergen_git_sha.to_string(),
        build_timestamp: meta.vergen_build_timestamp.to_string(),
    }
}

/// Get a reference to the global version metadata.
///
/// Returns [`unknown_version_metadata`] when no binary set it with
/// [`try_init_version_metadata`].
pub fn version_metadata() -> &'static RethCliVersionConsts {
    VERSION_METADATA.get_or_init(unknown_version_metadata)
}

/// Version metadata without anything a build script stamps: the package version and `unknown`
/// for the rest.
pub fn unknown_version_metadata() -> RethCliVersionConsts {
    let version = env!("CARGO_PKG_VERSION");
    let unknown = || Cow::Borrowed("unknown");
    RethCliVersionConsts {
        name_client: Cow::Borrowed("Reth"),
        cargo_pkg_version: Cow::Borrowed(version),
        vergen_git_sha_long: unknown(),
        vergen_git_sha: unknown(),
        vergen_build_timestamp: unknown(),
        vergen_cargo_target_triple: unknown(),
        vergen_cargo_features: unknown(),
        short_version: Cow::Borrowed(version),
        long_version: Cow::Owned(format!("Version: {version}")),
        build_profile_name: unknown(),
        p2p_client_version: Cow::Owned(format!("reth/v{version}/{}", std::env::consts::OS)),
        extra_data: Cow::Owned(default_extra_data()),
    }
}

/// Reth's version metadata, stamped by the calling binary's build script.
///
/// Version metadata changes with every commit, and the build timestamp with every build. Stamped
/// in this crate, it would rebuild every crate that depends on this one. Instead, a binary calls
/// `reth_node_version_build::emit_version_metadata` from its build script, and sets the result
/// with [`try_init_version_metadata`]:
///
/// ```ignore
/// reth_node_core::version::try_init_version_metadata(reth_node_core::reth_version_metadata!())
///     .expect("version metadata is set once, before anything reads it");
/// ```
///
/// [`try_init_version_metadata`]: crate::version::try_init_version_metadata
#[macro_export]
macro_rules! reth_version_metadata {
    () => {
        $crate::version::RethCliVersionConsts {
            name_client: ::std::borrow::Cow::Borrowed("Reth"),
            cargo_pkg_version: ::std::borrow::Cow::Borrowed(env!("CARGO_PKG_VERSION")),
            vergen_git_sha_long: ::std::borrow::Cow::Borrowed(env!("VERGEN_GIT_SHA")),
            vergen_git_sha: ::std::borrow::Cow::Borrowed(env!("VERGEN_GIT_SHA_SHORT")),
            vergen_build_timestamp: ::std::borrow::Cow::Borrowed(env!("VERGEN_BUILD_TIMESTAMP")),
            vergen_cargo_target_triple: ::std::borrow::Cow::Borrowed(env!(
                "VERGEN_CARGO_TARGET_TRIPLE"
            )),
            vergen_cargo_features: ::std::borrow::Cow::Borrowed(env!("VERGEN_CARGO_FEATURES")),
            short_version: ::std::borrow::Cow::Borrowed(env!("RETH_SHORT_VERSION")),
            long_version: ::std::borrow::Cow::Borrowed(concat!(
                env!("RETH_LONG_VERSION_0"),
                "\n",
                env!("RETH_LONG_VERSION_1"),
                "\n",
                env!("RETH_LONG_VERSION_2"),
                "\n",
                env!("RETH_LONG_VERSION_3"),
                "\n",
                env!("RETH_LONG_VERSION_4"),
            )),
            build_profile_name: ::std::borrow::Cow::Borrowed(env!("RETH_BUILD_PROFILE")),
            p2p_client_version: ::std::borrow::Cow::Borrowed(env!("RETH_P2P_CLIENT_VERSION")),
            extra_data: ::std::borrow::Cow::Owned($crate::version::default_extra_data()),
        }
    };
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

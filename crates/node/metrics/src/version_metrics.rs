//! This exposes reth's version information over prometheus.
use metrics::gauge;

/// The build timestamp.
pub const VERGEN_BUILD_TIMESTAMP: &str = env!("VERGEN_BUILD_TIMESTAMP");
/// The cargo features enabled for the build.
pub const VERGEN_CARGO_FEATURES: &str = env!("VERGEN_CARGO_FEATURES");
/// The target triple for the build.
pub const VERGEN_CARGO_TARGET_TRIPLE: &str = env!("VERGEN_CARGO_TARGET_TRIPLE");
/// The full SHA of the latest commit.
pub const VERGEN_GIT_SHA_LONG: &str = env!("VERGEN_GIT_SHA");
/// The 8 character short SHA of the latest commit.
pub const VERGEN_GIT_SHA: &str = const_format::str_index!(VERGEN_GIT_SHA_LONG, ..8);

/// The build profile name.
pub const BUILD_PROFILE_NAME: &str = {
    // Derived from https://stackoverflow.com/questions/73595435/how-to-get-profile-from-cargo-toml-in-build-rs-or-at-runtime
    // We split on the path separator of the *host* machine, which may be different from
    // `std::path::MAIN_SEPARATOR_STR`.
    const OUT_DIR: &str = env!("OUT_DIR");
    let unix_parts = const_format::str_split!(OUT_DIR, '/');
    if unix_parts.len() >= 4 {
        unix_parts[unix_parts.len() - 4]
    } else {
        let win_parts = const_format::str_split!(OUT_DIR, '\\');
        win_parts[win_parts.len() - 4]
    }
};

/// Contains version information for the application.
#[derive(Debug, Clone)]
pub struct VersionInfo {
    /// The version of the application.
    pub version: &'static str,
    /// The build timestamp of the application.
    pub build_timestamp: &'static str,
    /// The cargo features enabled for the build.
    pub cargo_features: &'static str,
    /// The Git SHA of the build.
    pub git_sha: &'static str,
    /// The target triple for the build.
    pub target_triple: &'static str,
    /// The build profile (e.g., debug or release).
    pub build_profile: &'static str,
}

impl Default for VersionInfo {
    fn default() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION"),
            build_timestamp: VERGEN_BUILD_TIMESTAMP,
            cargo_features: VERGEN_CARGO_FEATURES,
            git_sha: VERGEN_GIT_SHA,
            target_triple: VERGEN_CARGO_TARGET_TRIPLE,
            build_profile: BUILD_PROFILE_NAME,
        }
    }
}

impl VersionInfo {
    /// This exposes reth's version information over prometheus.
    pub fn register_version_metrics(&self) {
        let labels: [(&str, &str); 6] = [
            ("version", self.version),
            ("build_timestamp", self.build_timestamp),
            ("cargo_features", self.cargo_features),
            ("git_sha", self.git_sha),
            ("target_triple", self.target_triple),
            ("build_profile", self.build_profile),
        ];

        let gauge = gauge!("info", &labels);
        gauge.set(1)
    }
}

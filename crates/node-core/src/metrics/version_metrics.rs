//! This exposes reth's version information over prometheus.

use crate::version::{ build_profile_name, VERGEN_GIT_SHA };
use metrics::gauge;

pub struct VersionInfo {
    version: &str,
    build_timestamp: &str,
    cargo_features: &str,
    git_sha: &str,
    target_triple: &str,
    build_profile: &str,
}

impl Default for VersionInfo {
    fn default() -> Self {
        VersionInfo {
            version: env!("CARGO_PKG_VERSION"),
            build_timestamp: env!("VERGEN_BUILD_TIMESTAMP"),
            cargo_features: env!("VERGEN_CARGO_FEATURES"),
            git_sha: env!("VERGEN_GIT_SHA"),
            target_triple: env!("VERGEN_CARGO_TARGET_TRIPLE"),
            build_profile: build_profile_name(),
        }
    }
}

/// This exposes reth's version information over prometheus.
pub fn register_version_metrics(version_info: VersionInfo) {
    let labels: [(&str, &str); 6] = [
        ("version", version_info.version),
        ("build_timestamp", version_info.build_timestamp),
        ("cargo_features", version_info.cargo_features),
        ("git_sha", version_info.git_sha),
        ("target_triple", version_info.target_triple),
        ("build_profile", version_info.build_profile),
    ];

    let _gauge = gauge!("info", &labels);
}

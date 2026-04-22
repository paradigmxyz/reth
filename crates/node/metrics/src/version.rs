//! This exposes reth's version information over prometheus.
use metrics::gauge;
use reth_node_core::version::version_metadata;

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

impl VersionInfo {
    /// Creates a new [`VersionInfo`] from the reth version metadata.
    pub fn from_reth_metadata() -> Self {
        Self {
            version: version_metadata().cargo_pkg_version.as_ref(),
            build_timestamp: version_metadata().vergen_build_timestamp.as_ref(),
            cargo_features: version_metadata().vergen_cargo_features.as_ref(),
            git_sha: version_metadata().vergen_git_sha.as_ref(),
            target_triple: version_metadata().vergen_cargo_target_triple.as_ref(),
            build_profile: version_metadata().build_profile_name.as_ref(),
        }
    }

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
        gauge.set(1);
    }
}

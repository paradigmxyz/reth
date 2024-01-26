//! This exposes reth's version information over prometheus.

use crate::version::build_profile_name;
use metrics::register_gauge;

const LABELS: [(&str, &str); 6] = [
    ("version", env!("CARGO_PKG_VERSION")),
    ("build_timestamp", env!("VERGEN_BUILD_TIMESTAMP")),
    ("cargo_features", env!("VERGEN_CARGO_FEATURES")),
    ("git_sha", env!("VERGEN_GIT_SHA")),
    ("target_triple", env!("VERGEN_CARGO_TARGET_TRIPLE")),
    ("build_profile", build_profile_name()),
];

/// This exposes reth's version information over prometheus.
pub fn register_version_metrics() {
    register_gauge!("info", &LABELS);
}

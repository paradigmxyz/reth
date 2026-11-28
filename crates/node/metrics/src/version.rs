//! This exposes reth's version information over prometheus.
use metrics::gauge;

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

#[cfg(test)]
mod tests {
    use super::*;
    use metrics_exporter_prometheus::PrometheusBuilder;
    use metrics_util::layers::{Layer, PrefixLayer};

    /// Metric names referenced by production alerts. Renaming any of these silently breaks
    /// alerting, so the exported Prometheus name and `git_sha` label are pinned here.
    #[test]
    fn alerted_metric_names() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&PrefixLayer::new("reth").layer(recorder), || {
            VersionInfo {
                version: "1.0.0",
                build_timestamp: "2026-01-01T00:00:00Z",
                cargo_features: "",
                git_sha: "abcdef",
                target_triple: "x86_64-unknown-linux-gnu",
                build_profile: "release",
            }
            .register_version_metrics();
        });
        let rendered = handle.render();

        assert!(
            rendered
                .lines()
                .any(|line| line.starts_with("reth_info{") && line.contains(r#"git_sha="abcdef""#)),
            "missing `reth_info` in:\n{rendered}"
        );
    }
}

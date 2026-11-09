use crate::StageId;
use reth_metrics::{metrics::Gauge, Metrics};
use std::collections::HashMap;

#[derive(Debug, Default)]
pub(crate) struct SyncMetrics {
    /// Stage metrics by stage.
    pub(crate) stages: HashMap<StageId, StageMetrics>,
}

impl SyncMetrics {
    /// Returns existing or initializes a new instance of [`StageMetrics`] for the provided
    /// [`StageId`].
    pub(crate) fn get_stage_metrics(&mut self, stage_id: StageId) -> &mut StageMetrics {
        self.stages
            .entry(stage_id)
            .or_insert_with(|| StageMetrics::new_with_labels(&[("stage", stage_id.to_string())]))
    }
}

#[derive(Metrics)]
#[metrics(scope = "sync")]
pub(crate) struct StageMetrics {
    /// The block number of the last commit for a stage.
    pub(crate) checkpoint: Gauge,
    /// The number of processed entities of the last commit for a stage, if applicable.
    pub(crate) entities_processed: Gauge,
    /// The number of total entities of the last commit for a stage, if applicable.
    pub(crate) entities_total: Gauge,
    /// The number of seconds spent executing the stage and committing the data.
    pub(crate) total_elapsed: Gauge,
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrics_exporter_prometheus::PrometheusBuilder;
    use metrics_util::layers::{Layer, PrefixLayer};

    /// Metric names referenced by production alerts. Renaming any of these silently breaks
    /// alerting, so the exported Prometheus names and stage labels are pinned here.
    #[test]
    fn alerted_metric_names() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&PrefixLayer::new("reth").layer(recorder), || {
            SyncMetrics::default().get_stage_metrics(StageId::Finish);
        });
        let rendered = handle.render();

        let sample = r#"reth_sync_checkpoint{stage="Finish"}"#;
        assert!(
            rendered.lines().any(|line| line.split(' ').next() == Some(sample)),
            "missing `{sample}` in:\n{rendered}"
        );
    }
}

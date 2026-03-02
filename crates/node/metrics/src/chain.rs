//! This exposes reth's chain information over prometheus.
use metrics::{describe_gauge, gauge};

/// Fork activation information for metrics.
#[derive(Debug, Clone)]
pub struct ForkActivation {
    /// Name of the fork.
    pub name: String,
    /// Type of activation condition ("block", "timestamp", "ttd", or "never").
    pub condition_type: String,
    /// Activation value (block number or timestamp). None for never-activated forks.
    pub activation_value: Option<u64>,
}

/// Contains chain information for the application.
#[derive(Debug, Clone)]
pub struct ChainSpecInfo {
    /// The name of the chain.
    pub name: String,
    /// Fork schedule with activation conditions.
    pub forks: Vec<ForkActivation>,
}

impl ChainSpecInfo {
    /// This exposes reth's chain information over prometheus.
    pub fn register_chain_spec_metrics(&self) {
        let labels: [(&str, String); 1] = [("name", self.name.clone())];

        describe_gauge!("chain_spec", "Information about the chain");
        let _gauge = gauge!("chain_spec", &labels);

        describe_gauge!(
            "chain_spec.fork",
            "Fork activation value (block number or timestamp). Label 'type' indicates block/timestamp/ttd/never."
        );
        for fork in &self.forks {
            let fork_labels: [(&str, String); 3] = [
                ("fork", fork.name.clone()),
                ("type", fork.condition_type.clone()),
                ("chain", self.name.clone()),
            ];
            let g = gauge!("chain_spec.fork", &fork_labels);
            if let Some(value) = fork.activation_value {
                g.set(value as f64);
            }
        }
    }
}

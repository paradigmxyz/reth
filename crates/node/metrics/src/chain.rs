//! This exposes reth's chain information over prometheus.
use metrics::{describe_gauge, gauge};
use reth_ethereum_forks::{ForkCondition, Hardforks};

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
    /// Builds [`ChainSpecInfo`] from any [`Hardforks`] implementation, capturing all configured
    /// forks including custom ones.
    pub fn from_hardforks(name: String, hardforks: &impl Hardforks) -> Self {
        Self {
            name,
            forks: hardforks
                .forks_iter()
                .map(|(fork, condition)| {
                    let (condition_type, activation_value) = match condition {
                        ForkCondition::Block(block) => ("block".to_string(), Some(block)),
                        ForkCondition::TTD { activation_block_number, .. } => {
                            ("ttd".to_string(), Some(activation_block_number))
                        }
                        ForkCondition::Timestamp(ts) => ("timestamp".to_string(), Some(ts)),
                        ForkCondition::Never => ("never".to_string(), None),
                    };
                    ForkActivation {
                        name: fork.name().to_string(),
                        condition_type,
                        activation_value,
                    }
                })
                .collect(),
        }
    }

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

//! This exposes reth's chain information over prometheus.
use metrics::{describe_gauge, gauge};

/// Contains chain information for the application.
#[derive(Debug, Clone)]
pub struct ChainSpecInfo {
    /// The name of the chain.
    pub name: String,
}

impl ChainSpecInfo {
    /// This exposes reth's chain information over prometheus.
    pub fn register_chain_spec_metrics(&self) {
        let labels: [(&str, String); 1] = [("name", self.name.clone())];

        describe_gauge!("chain_spec", "Information about the chain");
        let _gauge = gauge!("chain_spec", &labels);
    }
}

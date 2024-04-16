use std::sync::Arc;

use reth_primitives::{ChainSpec, Genesis};

/// Helper struct to configure the chain spec as needed for e2e tests
#[must_use = "call `build` to construct the chainspec"]
pub struct ChainSpecBuilder {
    chain_spec: ChainSpec,
}

impl ChainSpecBuilder {
    /// Creates a new chain spec builder with the static genesis.json
    pub fn new() -> Self {
        let genesis: Genesis =
            serde_json::from_str(include_str!("../assets/genesis.json")).unwrap();

        Self { chain_spec: genesis.into() }
    }

    /// Overrides the genesis block with the given one
    #[allow(unused)]
    pub fn with_genesis(mut self, genesis: Genesis) -> Self {
        self.chain_spec.genesis = genesis;
        self
    }

    /// Builds the chain spec
    pub fn build(self) -> Arc<ChainSpec> {
        Arc::new(self.chain_spec)
    }
}

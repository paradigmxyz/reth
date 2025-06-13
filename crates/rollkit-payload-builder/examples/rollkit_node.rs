use custom_payload_builder::{
    create_rollkit_node, create_rollkit_node_with_config, RollkitPayloadBuilderConfig,
};
use reth_node_core::node_config::NodeConfig;
use reth_provider::StateProviderFactory;
use std::sync::Arc;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    // Create a default node configuration
    let config = NodeConfig::default();

    // Create a client (this is a placeholder - you'll need to implement your own client)
    let client = Arc::new(YourClient::new());

    // Create a node with default configuration
    let node = create_rollkit_node(client.clone()).await?;

    // Or create a node with custom configuration
    let custom_config = RollkitPayloadBuilderConfig {
        max_transactions: 2000,
        max_gas_limit: 40_000_000,
        min_gas_price: 2_000_000_000,
        enable_tx_validation: true,
    };
    let custom_node = create_rollkit_node_with_config(client, custom_config).await?;

    // Start the node
    node.start().await?;

    Ok(())
}

// Placeholder for your client implementation
struct YourClient;

impl YourClient {
    fn new() -> Self {
        Self
    }
}

impl StateProviderFactory for YourClient {
    type Provider = Self;

    fn state_provider(&self) -> eyre::Result<Self::Provider> {
        Ok(self.clone())
    }
}

impl Clone for YourClient {
    fn clone(&self) -> Self {
        Self
    }
} 
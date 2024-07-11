//! GPU Art Execution Extension (ExEx) for Reth
//!
//! This crate implements a GPU-accelerated particle art generator as an Execution Extension for Reth.
//! It generates unique particle art for each new block in the Ethereum chain using Metal GPU acceleration.

use reth_node_ethereum::EthereumNode;
mod gpu_art_exex;

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("GPU-Art", gpu_art_exex::init_gpu_art_exex)
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}

#[cfg(test)]
mod tests {
    use super::gpu_art_exex;
    use reth_execution_types::{Chain, ExecutionOutcome};
    use reth_exex_test_utils::{test_exex_context, PollOnce};
    use std::fs;
    use std::path::Path;
    use std::pin::pin;

    #[tokio::test]
    async fn test_exex() -> eyre::Result<()> {
        let (ctx, handle) = test_exex_context().await?;

        // Initialize the GPU ExEx
        let mut exex = pin!(gpu_art_exex::init_gpu_art_exex(ctx).await?);

        // Send a notification for a new block
        handle
            .send_notification_chain_committed(Chain::from_block(
                handle.genesis.clone(),
                ExecutionOutcome::default(),
                None,
            ))
            .await?;

        // Poll the ExEx once to process the notification
        exex.poll_once().await?;

        // Check if the image file was created
        let expected_file = Path::new("gpu_art").join("block_0.png");
        assert!(expected_file.exists(), "GPU art file was not created");

        // Clean up the created file
        fs::remove_file(expected_file)?;

        Ok(())
    }
}

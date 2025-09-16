//! Actions for testing trie-related functionality, particularly for verifying
//! trie updates are properly preserved in canonical blocks.

use crate::testsuite::{actions::Action, Environment};
use eyre::Result;
use reth_node_api::EngineTypes;
use std::{future::Future, pin::Pin, time::Duration};
use tokio::time::sleep;

/// Action that asserts whether a block has missing trie updates
#[derive(Debug)]
pub struct AssertMissingTrieUpdates {
    block_tag: String,
    expect_missing: bool,
}

impl AssertMissingTrieUpdates {
    /// Create a new assertion for checking trie updates
    pub fn new(tag: impl Into<String>) -> Self {
        Self { block_tag: tag.into(), expect_missing: false }
    }

    /// Set whether we expect trie updates to be missing
    pub const fn expect_missing(mut self, missing: bool) -> Self {
        self.expect_missing = missing;
        self
    }
}

impl<Engine> Action<Engine> for AssertMissingTrieUpdates
where
    Engine: EngineTypes,
{
    fn execute<'a>(
        &'a mut self,
        env: &'a mut Environment<Engine>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            // Wait for any pending events
            sleep(Duration::from_millis(150)).await;
            env.drain_trie_updates();

            // Get block info
            let (block_info, node_idx) = *env
                .block_registry
                .get(&self.block_tag)
                .ok_or_else(|| eyre::eyre!("Unknown block tag {}", self.block_tag))?;

            eprintln!("\n=== CHECKING TRIE UPDATES STATUS FOR BLOCK {} ===", self.block_tag);
            eprintln!("Block hash: {}", block_info.hash);
            eprintln!("Block number: {}", block_info.number);

            let node_state = env.node_state(node_idx)?;

            // Check if block has trie updates
            let has_trie_updates = node_state.block_trie_updates.contains_key(&block_info.hash);

            if has_trie_updates {
                let updates = &node_state.block_trie_updates[&block_info.hash];
                let is_empty = updates.account_nodes.is_empty() &&
                    updates.storage_tries.is_empty() &&
                    updates.removed_nodes.is_empty();

                if is_empty {
                    eprintln!("⚠️  Block has EMPTY trie updates (all fields empty)");
                    eprintln!("    This is effectively the same as ExecutedTrieUpdates::Missing");

                    if !self.expect_missing {
                        return Err(eyre::eyre!(
                            "Expected block {} to have trie updates, but they are empty (effectively missing)",
                            self.block_tag
                        ));
                    }
                } else {
                    eprintln!("✅ Block has PRESENT trie updates:");
                    eprintln!("    Account nodes: {}", updates.account_nodes.len());
                    eprintln!("    Storage tries: {}", updates.storage_tries.len());
                    eprintln!("    Removed nodes: {}", updates.removed_nodes.len());

                    if self.expect_missing {
                        return Err(eyre::eyre!(
                            "Expected block {} to have MISSING trie updates, but found present updates",
                            self.block_tag
                        ));
                    }
                }
            } else {
                error!("Block has NO trie updates (ExecutedTrieUpdates::Missing)");

                if !self.expect_missing {
                    return Err(eyre::eyre!(
                        "Expected block {} to have trie updates, but they are MISSING (ExecutedTrieUpdates::Missing)",
                        self.block_tag
                    ));
                }
            }

            eprintln!("=== END TRIE UPDATES STATUS CHECK ===\n");

            Ok(())
        })
    }
}

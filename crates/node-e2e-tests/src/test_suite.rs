use crate::{chain_spec_builder::ChainSpecBuilder, wallet::Wallet};
use alloy_network::eip2718::Encodable2718;
use reth_primitives::{Bytes, ChainSpec, B256};
use std::sync::Arc;

/// Mnemonic used to derive the test accounts
const TEST_MNEMONIC: &str = "test test test test test test test test test test test junk";

/// Helper struct to customize the chain spec during e2e tests
pub struct TestSuite {
    wallet: Wallet,
}

impl Default for TestSuite {
    fn default() -> Self {
        Self::new()
    }
}

impl TestSuite {
    /// Creates a new e2e test suite with a test account prefunded with 10_000 ETH from genesis
    /// allocations and the eth mainnet latest chainspec.
    pub fn new() -> Self {
        let wallet = Wallet::new(TEST_MNEMONIC);
        Self { wallet }
    }

    /// Creates a signed transfer tx and returns its hash and raw bytes
    pub async fn transfer_tx(&self) -> (B256, Bytes) {
        let tx = self.wallet.transfer_tx().await;
        (tx.trie_hash(), tx.encoded_2718().into())
    }

    /// Chain spec for e2e eth tests
    ///
    /// Includes 20 prefunded accounts with 10_000 ETH each derived from mnemonic "test test test
    /// test test test test test test test test junk".
    pub fn chain_spec(&self) -> Arc<ChainSpec> {
        ChainSpecBuilder::new().build()
    }
}

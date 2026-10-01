//! Shared fixtures for the network integration tests.

use alloy_primitives::U256;
use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
use reth_transaction_pool::{
    test_utils::TransactionGenerator, EthPooledTransaction, PoolTransaction,
};
use std::time::Duration;

/// How long a test may wait for a condition, e.g. a session or an announcement.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Returns a transaction from a new sender that is funded in the provider.
pub(crate) fn funded_transaction(provider: &MockEthProvider) -> EthPooledTransaction {
    let tx = TransactionGenerator::with_num_signers(rand::rng(), 1).gen_eip1559_pooled();
    provider.add_account(tx.sender(), ExtendedAccount::new(0, U256::from(100_000_000)));
    tx
}

/// Returns transactions from distinct new senders that are funded in the provider.
pub(crate) fn funded_transactions(
    provider: &MockEthProvider,
    count: usize,
) -> Vec<EthPooledTransaction> {
    (0..count).map(|_| funded_transaction(provider)).collect()
}

/// Polls `f` until it returns `Some`, panicking if `condition` is not met within [`TIMEOUT`].
pub(crate) async fn poll_until<T>(condition: &str, mut f: impl AsyncFnMut() -> Option<T>) -> T {
    tokio::time::timeout(TIMEOUT, async {
        loop {
            if let Some(value) = f().await {
                return value
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {condition}"))
}

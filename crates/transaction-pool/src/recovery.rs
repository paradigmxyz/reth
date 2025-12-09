use alloy_consensus::crypto::RecoveryError;
use reth_primitives_traits::{Recovered, SignerRecoverable};
use tokio::sync::Semaphore;

// We allow up to 1000 concurrent conversions to avoid excessive memory usage.
static SEMAPHORE: Semaphore = Semaphore::const_new(5);

/// A simple semaphore-based blob sidecar converter.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct TxRecoveryHandle;

impl TxRecoveryHandle {
    /// Creates a new transaction recovery handle.
    pub const fn new() -> Self {
        Self
    }

    /// Recovers a [`SignerRecoverable`] transaction.
    pub async fn try_into_recovered<T: SignerRecoverable + Send + 'static>(
        &self,
        tx: T,
    ) -> Result<Recovered<T>, RecoveryError> {
        let _permit = SEMAPHORE.acquire().await.unwrap();
        tokio::task::spawn_blocking(move || tx.try_into_recovered())
            .await
            .expect("transaction recovery panicked")
    }
}

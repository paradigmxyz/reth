use alloy_consensus::{BlobTransactionSidecar, EnvKzgSettings};
use alloy_eips::eip7594::BlobTransactionSidecarEip7594;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// A simple semaphore-based blob sidecar converter.
#[derive(Debug, Clone)]
pub struct BlobSidecarConverter {
    sem: Arc<Semaphore>,
}

impl BlobSidecarConverter {
    /// Creates a new blob sidecar converter.
    pub fn new(max_concurrency: usize) -> Self {
        Self { sem: Arc::new(Semaphore::new(max_concurrency)) }
    }

    /// Converts the blob sidecar to the EIP-7594 format.
    pub async fn convert(
        &self,
        sidecar: BlobTransactionSidecar,
    ) -> Option<BlobTransactionSidecarEip7594> {
        let _permit = self.sem.acquire().await.ok()?;
        tokio::task::spawn_blocking(move || sidecar.try_into_7594(EnvKzgSettings::Default.get()))
            .await
            .ok()?
            .ok()
    }
}

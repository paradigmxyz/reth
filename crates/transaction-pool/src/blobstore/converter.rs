use alloy_consensus::{BlobTransactionSidecar, EnvKzgSettings};
use alloy_eips::eip7594::BlobTransactionSidecarEip7594;
use tokio::sync::Semaphore;

// We allow up to 5 concurrent conversions to avoid excessive memory usage.
static SEMAPHORE: Semaphore = Semaphore::const_new(5);

/// A simple semaphore-based blob sidecar converter.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct BlobSidecarConverter;

impl BlobSidecarConverter {
    /// Creates a new blob sidecar converter.
    pub const fn new() -> Self {
        Self
    }

    /// Converts the blob sidecar to the EIP-7594 format.
    pub async fn convert(
        &self,
        sidecar: BlobTransactionSidecar,
    ) -> Option<BlobTransactionSidecarEip7594> {
        let _permit = SEMAPHORE.acquire().await.ok()?;
        tokio::task::spawn_blocking(move || sidecar.try_into_7594(EnvKzgSettings::Default.get()))
            .await
            .ok()?
            .ok()
    }
}

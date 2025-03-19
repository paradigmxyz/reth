//! This is our custom implementation of validator struct

use alloy_rpc_client::ReqwestClient;
pub use kona_interop::{ExecutingDescriptor, SafetyLevel};
pub use kona_rpc::{
    CheckAccessList, InteropTxValidator, InteropTxValidatorError, SupervisorApiClient,
};
use std::time::Duration;
use alloc::string::String;

/// Implementation of the supervisor trait for the interop.
#[derive(Debug, Clone)]
pub struct SupervisorClient {
    inner: kona_rpc::SupervisorClient,
    safety: SafetyLevel,
}

impl SupervisorClient {
    /// Creates a new supervisor validator.
    pub async fn new(supervisor_endpoint: impl Into<String>, safety: Option<String>) -> Self {
        let inner = kona_rpc::SupervisorClient::new(
            ReqwestClient::builder()
                .connect(supervisor_endpoint.into().as_str())
                .await
                .expect("building supervisor client"),
        );
        let safety = safety
            .map(|safety| {
                serde_json::from_str(safety.as_str()).expect("invalid safety level provided")
            })
            .unwrap_or(SafetyLevel::CrossUnsafe);
        Self { inner, safety }
    }

    /// Returns safely level
    pub fn safety(&self) -> SafetyLevel {
        self.safety
    }
}

impl InteropTxValidator for SupervisorClient {
    type SupervisorClient = kona_rpc::SupervisorClient;
    const DEFAULT_TIMEOUT: Duration = Duration::from_millis(100);

    fn supervisor_client(&self) -> &Self::SupervisorClient {
        &self.inner
    }
}

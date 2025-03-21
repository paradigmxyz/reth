//! This is our custom implementation of validator struct

use alloc::string::String;
use alloy_rpc_client::ReqwestClient;
pub use kona_interop::{ExecutingDescriptor, InvalidInboxEntry, SafetyLevel};
pub use kona_rpc::{
    CheckAccessList, InteropTxValidator, InteropTxValidatorError, SupervisorApiClient,
};
use std::time::Duration;

/// Supervisor hosted by op-labs
// TODO: This should be changes to actual supervisor url
pub const DEFAULT_SUPERVISOR_URL: &str = "http://localhost:1337/";

/// Implementation of the supervisor trait for the interop.
#[derive(Debug, Clone)]
pub struct SupervisorClient {
    inner: kona_rpc::SupervisorClient,
    safety: SafetyLevel,
}

impl SupervisorClient {
    /// Creates a new supervisor validator.
    pub async fn new(supervisor_endpoint: impl Into<String>, safety: SafetyLevel) -> Self {
        let inner = kona_rpc::SupervisorClient::new(
            ReqwestClient::builder()
                .connect(supervisor_endpoint.into().as_str())
                .await
                .expect("building supervisor client"),
        );
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

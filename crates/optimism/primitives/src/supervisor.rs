//! This is our custom implementation of validator struct

use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use std::time::Duration;
use url::Url;

pub use kona_interop::{ExecutingDescriptor, SafetyLevel};
pub use kona_rpc::{InteropTxValidator, InteropTxValidatorError};

/// Implementation of the supervisor trait for the interop.
#[derive(Debug, Clone)]
pub struct SupervisorClient {
    client: HttpClient,
    safety: SafetyLevel,
}

impl SupervisorClient {
    /// Creates a new supervisor validator.
    pub fn new(supervisor_endpoint: impl Into<String>, safety: Option<String>) -> Self {
        let url = Url::parse(supervisor_endpoint.into().as_str()).expect("parsing supervisor url");
        let client =
            HttpClientBuilder::default().build(url).expect("building supervisor http client");
        let safety = safety
            .map(|safety| {
                serde_json::from_str(safety.as_str()).expect("invalid safety level provided")
            })
            .unwrap_or(SafetyLevel::CrossUnsafe);
        Self { client, safety }
    }

    /// Returns safely level
    pub fn safety(&self) -> SafetyLevel {
        self.safety
    }
}

impl InteropTxValidator for SupervisorClient {
    type SupervisorClient = HttpClient;
    const DEFAULT_TIMEOUT: Duration = Duration::from_millis(100);

    fn supervisor_client(&self) -> &Self::SupervisorClient {
        &self.client
    }
}

//! This is our custom implementation of validator struct

use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use kona_interop::{ExecutingDescriptor, SafetyLevel};
use kona_rpc::InteropTxValidator;
use std::{sync::Arc, time::Duration};
/// Implementation of the supervisor trait for the interop.
#[derive(Debug, Clone)]
pub struct SupervisorClient(HttpClient);

impl SupervisorClient {
    /// Creates a new supervisor validator.
    pub fn new(supervisor_endpoint: impl Into<String>) -> Self {
        let url = Url::parse(supervisor_endpoint.into()).expect("parsing supervisor url");
        let client =
            HttpClientBuilder::default().build(url).expect("building supervisor http client");
        Self(client)
    }
}


impl InteropTxValidator for SupervisorClientInner {
    type SupervisorClient = HttpClient;
    const DEFAULT_TIMEOUT: Duration = Duration::from_millis(100);

    fn supervisor_client(&self) -> &Self::SupervisorClient {
        &self.0
    }
}

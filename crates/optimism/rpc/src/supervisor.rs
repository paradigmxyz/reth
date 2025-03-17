//! This is our custom implementation of validator struct

use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use kona_interop::{ExecutingDescriptor, SafetyLevel};
use kona_rpc::InteropTxValidator;
use std::{sync::Arc, time::Duration};
/// Implementation of the supervisor trait for the interop.
#[derive(Debug, Clone)]
pub struct SupervisorClient {
    inner: Arc<SupervisorClientInner>,
}

impl SupervisorClient {
    /// Creates a new supervisor validator.
    pub fn new(supervisor_endpoint: impl Into<String>) -> Self {
        let url = Url::parse(supervisor_endpoint.into()).expect("parsing supervisor url");
        let client =
            HttpClientBuilder::default().build(url).expect("building supervisor http client");
        Self::with_client(supervisor_endpoint, client)
    }

    /// Creates a new supervisor validator.
    pub fn with_client(supervisor_endpoint: impl Into<String>, client: HttpClient) -> Self {
        let inner =
            SupervisorClientInner { supervisor_endpoint: supervisor_endpoint.into(), client };
        Self { inner: Arc::new(inner) }
    }

    pub fn parse_access_list(access_list_items: &[AccessListItem]) -> impl Iterator<Item = &B256> {
        self.inner.parse_access_list(access_list_items)
    }

    /// Validates a list of inbox entries against a Supervisor.
    pub async fn validate_messages(
        &self,
        inbox_entries: &[B256],
        safety: SafetyLevel,
        executing_descriptor: ExecutingDescriptor,
        timeout: Option<Duration>,
    ) -> Result<(), InteropTxValidatorError> {
        self.inner.validate_messages(inbox_entries, safety, executing_descriptor, timeout).await
    }
}

pub struct SupervisorClientInner {
    /// The endpoint of the supervisor
    supervisor_endpoint: String,
    /// The HTTP client
    client: HttpClient,
}

impl InteropTxValidator for SupervisorClientInner {
    type SupervisorClient = HttpClient;
    const DEFAULT_TIMEOUT: Duration = Duration::from_millis(100);

    fn supervisor_client(&self) -> &Self::SupervisorClient {
        &self.client
    }
}

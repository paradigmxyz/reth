//! Helpers for testing debug trace calls.

use reth_primitives::H256;
use reth_rpc_api::clients::DebugApiClient;
use reth_rpc_types::trace::geth::{GethDebugTracerType, GethDebugTracingOptions};

const NOOP_TRACER: &str = include_str!("../assets/noop-tracer.js");

/// An extension trait for the Trace API.
#[async_trait::async_trait]
pub trait DebugApiExt {
    /// The provider type that is used to make the requests.
    type Provider;

    /// Same as [DebugApiClient::debug_trace_transaction] but returns the result as json.
    async fn debug_trace_transaction_json(
        &self,
        hash: H256,
        opts: GethDebugTracingOptions,
    ) -> Result<serde_json::Value, jsonrpsee::core::Error>;
}

#[async_trait::async_trait]
impl<T: DebugApiClient + Sync> DebugApiExt for T {
    type Provider = T;

    async fn debug_trace_transaction_json(
        &self,
        hash: H256,
        opts: GethDebugTracingOptions,
    ) -> Result<serde_json::Value, jsonrpsee::core::Error> {
        let mut params = jsonrpsee::core::params::ArrayParams::new();
        params.insert(hash).unwrap();
        params.insert(opts).unwrap();
        self.request("debug_traceTransaction", params).await
    }
}

/// A javascript tracer that does nothing
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct NoopJsTracer;

impl From<NoopJsTracer> for GethDebugTracingOptions {
    fn from(_: NoopJsTracer) -> Self {
        GethDebugTracingOptions {
            tracer: Some(GethDebugTracerType::JsTracer(NOOP_TRACER.to_string())),
            tracer_config: serde_json::Value::Object(Default::default()).into(),
            ..Default::default()
        }
    }
}
impl From<NoopJsTracer> for Option<GethDebugTracingOptions> {
    fn from(_: NoopJsTracer) -> Self {
        Some(NoopJsTracer.into())
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        debug::{DebugApiExt, NoopJsTracer},
        utils::parse_env_url,
    };
    use jsonrpsee::http_client::HttpClientBuilder;

    #[tokio::test]
    #[ignore]
    async fn can_trace_noop_sepolia() {
        // random tx <https://sepolia.etherscan.io/tx/0x5525c63a805df2b83c113ebcc8c7672a3b290673c4e81335b410cd9ebc64e085>
        let tx =
            "0x5525c63a805df2b83c113ebcc8c7672a3b290673c4e81335b410cd9ebc64e085".parse().unwrap();
        let url = parse_env_url("RETH_RPC_TEST_NODE_URL").unwrap();
        let client = HttpClientBuilder::default().build(url).unwrap();
        let res =
            client.debug_trace_transaction_json(tx, NoopJsTracer::default().into()).await.unwrap();
        assert_eq!(res, serde_json::Value::Object(Default::default()));
    }
}

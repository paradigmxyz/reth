//! Helpers for testing debug trace calls.

use reth_rpc_types::trace::geth::{GethDebugTracerConfig, GethDebugTracerType, GethDebugTracingOptions};
const NOOP_TRACER:&str = include_str!("../assets/noop-tracer.js");

/// A javascript tracer that does nothing
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct NoopJsTracer;

impl From<NoopJsTracer> for GethDebugTracingOptions {
    fn from(_: NoopJsTracer) -> Self {
        GethDebugTracingOptions {
            tracer: Some(GethDebugTracerType::JsTracer(NOOP_TRACER.to_string())),
            tracer_config: Some(GethDebugTracerConfig::JsTracer(serde_json::Value::Object(Default::default()))),
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
    use jsonrpsee::http_client::HttpClientBuilder;
    use crate::debug::NoopJsTracer;
    use crate::utils::parse_env_url;
    use reth_rpc_api::DebugApiClient;

    #[tokio::test]
    // #[ignore]
    async fn can_trace_noop_sepolia() {
        // random tx <https://sepolia.etherscan.io/tx/0x5525c63a805df2b83c113ebcc8c7672a3b290673c4e81335b410cd9ebc64e085>
        let tx = "0x5525c63a805df2b83c113ebcc8c7672a3b290673c4e81335b410cd9ebc64e085".parse().unwrap();
        let url = parse_env_url("RETH_RPC_TEST_NODE_URL").unwrap();
        let client = HttpClientBuilder::default().build(url).unwrap();
        let res = client.debug_trace_transaction(tx, NoopJsTracer::default().into()).await;
        dbg!(&res);
    }
}

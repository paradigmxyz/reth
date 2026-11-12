//! various serde test

use crate::utils::launch_http;
use alloy_primitives::U256;
use reth_rpc_server_types::RethRpcModule;
use serde_json::value::RawValue;

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_balance_serde() {
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let client = handle.http_client().unwrap();

    let params = RawValue::from_string(
        r#"["0xaa00000000000000000000000000000000000000","0x898753d8fdd8d92c1907ca21e68c7970abd290c647a202091181deec3f30a0b2"]"#
            .to_owned(),
    )
    .unwrap();

    client.request::<_, U256>("eth_getBalance", params).await.unwrap();
}

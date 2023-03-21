//! various serde test
use crate::utils::launch_http;
use jsonrpsee::{
    core::{client::ClientT, traits::ToRpcParams, Error},
    types::Request,
};
use reth_primitives::U256;
use reth_rpc_builder::RethRpcModule;
use serde_json::value::RawValue;

struct RawRpcParams(Box<RawValue>);

impl ToRpcParams for RawRpcParams {
    fn to_rpc_params(self) -> Result<Option<Box<RawValue>>, Error> {
        Ok(Some(self.0))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth_balance_serde() {
    let handle = launch_http(vec![RethRpcModule::Eth]).await;
    let s = r#"{"jsonrpc":"2.0","id":1,"method":"eth_getBalance","params":["0xaa00000000000000000000000000000000000000","0x898753d8fdd8d92c1907ca21e68c7970abd290c647a202091181deec3f30a0b2"]}"#;
    let req: Request = serde_json::from_str(s).unwrap();
    let client = handle.http_client().unwrap();

    let params = RawRpcParams(RawValue::from_string(req.params.unwrap().to_string()).unwrap());

    client.request::<U256, _>("eth_getBalance", params).await.unwrap();
}

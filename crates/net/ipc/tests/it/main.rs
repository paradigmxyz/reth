use jsonrpsee::{core::client::ClientT, rpc_params, RpcModule};
use parity_tokio_ipc::dummy_endpoint;
use reth_ipc::{client::IpcClientBuilder, server::Builder};

#[tokio::test]
#[cfg(unix)]
async fn test_say_hello() {
    let endpoint = dummy_endpoint();
    let server = Builder::default().build(&endpoint).unwrap();
    let mut module = RpcModule::new(());
    module.register_method("say_hello", |_, _| Ok("lo")).unwrap();
    let handle = server.start(module).unwrap();
    tokio::spawn(handle.stopped());

    let client = IpcClientBuilder::default().build(endpoint).await.unwrap();
    let response: String = client.request("say_hello", rpc_params![]).await.unwrap();
    dbg!(response);
}

fn main() {}

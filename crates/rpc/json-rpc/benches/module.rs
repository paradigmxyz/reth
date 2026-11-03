//! Benchmarks for dispatching requests through an [`RpcModule`].

#![allow(missing_docs)]

use criterion::{criterion_group, criterion_main, Criterion};
use futures::executor::block_on;
use reth_json_rpc::{rpc, RpcResult};
use std::hint::black_box;

#[rpc(server, namespace = "eth")]
trait Eth {
    #[method(name = "chainId")]
    fn chain_id(&self) -> RpcResult<u64>;

    #[method(name = "getBalance")]
    async fn balance(&self, address: String, block: Option<String>) -> RpcResult<String>;
}

struct EthImpl;

impl EthServer for EthImpl {
    fn chain_id(&self) -> RpcResult<u64> {
        Ok(1)
    }

    async fn balance(&self, address: String, block: Option<String>) -> RpcResult<String> {
        Ok(format!("{}{}", &address[..4], block.as_deref().unwrap_or_default()))
    }
}

fn module(c: &mut Criterion) {
    let module = EthImpl.into_rpc();
    let mut call = |name, req: String| {
        let module = module.clone();
        c.bench_function(name, move |b| {
            b.iter(|| block_on(module.raw_json_request(black_box(&req))).unwrap())
        });
    };
    call(
        "module/chain_id",
        r#"{"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}"#.into(),
    );
    call(
        "module/get_balance",
        r#"{"jsonrpc":"2.0","id":"a","method":"eth_getBalance","params":["0x95222290dd7278aa3ddd389cc1e1d165cc4bafe5","latest"]}"#.into(),
    );
    call("module/not_found", r#"{"jsonrpc":"2.0","id":1,"method":"eth_missing"}"#.into());
    let batch = (0..100)
        .map(|i| format!(r#"{{"jsonrpc":"2.0","id":{i},"method":"eth_chainId","params":[]}}"#))
        .collect::<Vec<_>>()
        .join(",");
    call("module/batch_100", format!("[{batch}]"));
}

criterion_group!(benches, module);
criterion_main!(benches);

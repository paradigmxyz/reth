use jsonrpsee::proc_macros::rpc;
use jsonrpsee::core::RpcResult;
#[rpc(client, server)]
pub trait DuplicatedAlias {
	#[method(name = "foo", aliases = ["foo_dup", "foo_dup"])]
	async fn async_method(&self) -> RpcResult<u8>;
}

fn main() {}

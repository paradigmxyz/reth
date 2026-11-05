use jsonrpsee::{proc_macros::rpc, core::RpcResult};

// Names must be unique.
#[rpc(client, server)]
pub trait MethodNameConflict {
	#[method(name = "foo")]
	async fn foo(&self) -> RpcResult<u8>;

	#[method(name = "foo")]
	async fn bar(&self) -> RpcResult<u8>;
}

fn main() {}

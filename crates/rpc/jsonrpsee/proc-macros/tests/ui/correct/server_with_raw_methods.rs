//! Example of using proc macro to generate working client.

use jsonrpsee::{core::RpcResult, proc_macros::rpc, types::ErrorObjectOwned};

#[rpc(server)]
pub trait Rpc {
	#[method(name = "foo")]
	async fn async_method(&self, param_a: u8, param_b: String) -> RpcResult<u16>;

	#[method(name = "bar")]
	fn sync_method(&self) -> Result<u16, ErrorObjectOwned>;
}

fn main() {}

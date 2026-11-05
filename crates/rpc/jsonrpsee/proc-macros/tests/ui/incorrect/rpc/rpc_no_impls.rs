use jsonrpsee::proc_macros::rpc;

// Either client or server field must be provided.
#[rpc()]
pub trait NoImpls {
	#[method(name = "foo")]
	async fn async_method(&self) -> jsonrpsee::core::RpcResult<u8>;
}

fn main() {}

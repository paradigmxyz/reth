use jsonrpsee::proc_macros::rpc;

// Associated items are forbidden.
#[rpc(client, server)]
pub trait AssociatedConst {
	const WOO: usize;

	#[method(name = "foo")]
	async fn async_method(&self) -> jsonrpsee::core::RpcResult<u8>;
}

#[rpc(client, server)]
pub trait AssociatedType {
	type Woo;

	#[method(name = "foo")]
	async fn async_method(&self) -> jsonrpsee::core::RpcResult<u8>;
}

fn main() {}

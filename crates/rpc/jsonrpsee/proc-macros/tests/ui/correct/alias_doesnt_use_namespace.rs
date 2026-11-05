use jsonrpsee::{core::RpcResult, proc_macros::rpc};

#[rpc(client, server, namespace = "myapi")]
pub trait Rpc {
	/// Aliases don't use the namespace.
	/// Thus, this will generate `myapi_getTemp` and `getTemp`.
	#[method(name = "getTemp", aliases = ["getTemp"])]
	async fn async_method(&self, param_a: u8, param_b: String) -> RpcResult<u16>;

	#[subscription(name = "subscribeGetFood", item = String, aliases = ["getFood"], unsubscribe_aliases = ["unsubscribegetFood"])]
	async fn sub(&self);
}

fn main() {}

//! Example of using proc macro to generate working client.

use jsonrpsee::proc_macros::rpc;

#[rpc(client)]
pub trait Rpc {
	#[method(name = "foo")]
	async fn async_method(&self, param_a: u8, param_b: String) -> Result<u16, Error>;

	#[method(name = "bar")]
	fn sync_method(&self) -> Result<u16, Error>;

	#[subscription(name = "subscribe", item = String)]
	async fn sub(&self) -> Result<(), Error>;
}

fn main() {}

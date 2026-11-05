use jsonrpsee::proc_macros::rpc;

// Missing mandatory `name` field.
#[rpc(client, server)]
pub trait NoSubName {
	#[subscription(item = String)]
	async fn async_method(&self);
}

fn main() {}

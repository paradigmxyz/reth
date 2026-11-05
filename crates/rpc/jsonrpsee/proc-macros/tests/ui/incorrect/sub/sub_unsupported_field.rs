use jsonrpsee::proc_macros::rpc;

// Unsupported attribute field.
#[rpc(client, server)]
pub trait UnsupportedField {
	#[subscription(name = "sub", unsubscribe = "unsub", item = u8, magic = true)]
	async fn sub(&self);
}

fn main() {}

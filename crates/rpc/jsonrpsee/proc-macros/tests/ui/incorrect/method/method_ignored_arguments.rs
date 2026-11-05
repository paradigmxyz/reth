use jsonrpsee::proc_macros::rpc;

#[rpc(server)]
pub trait IgnoredArgument {
	#[method(name = "a")]
	async fn a(&self, _: Vec<u8>);
}

fn main() {}

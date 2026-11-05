use jsonrpsee::proc_macros::rpc;

#[rpc(client)]
pub trait NonResultReturnType {
	#[method(name = "a")]
	async fn a(&self) -> u16;
}

fn main() {}

use jsonrpsee::proc_macros::rpc;

fn main() {
	#[rpc(server)]
	pub trait Rpc {
		#[subscription(
			name = "submitAndWatchExtrinsic",
			unsubscribe = "author_unwatchExtrinsic",
			aliases = ["author_extrinsicUpdate"],
			unsubscribe_aliases = ["author_unwatchExtrinsic2"],
			// Arguments are being parsed the nearest comma,
			// angle braces need to be accounted for manually.
			item = TransactionStatus<Hash, BlockHash>,
		)]
		async fn dummy_subscription(&self);
	}
}

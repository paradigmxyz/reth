use jsonrpsee::proc_macros::rpc;

// Subscription method name conflict with notif override.
#[rpc(client, server)]
pub trait DupName {
	#[subscription(name = "one" => "one", unsubscribe = "unsubscribeOne", item = u8)]
	async fn one(&self);
}

fn main() {}

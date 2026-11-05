use jsonrpsee::proc_macros::rpc;

pub trait Config {
	type Hash: Send + Sync + 'static;
}

#[rpc(server, client_bounds(), server_bounds(Conf::Hash: jsonrpsee::core::Serialize))]
pub trait ClientBoundsForbidden<Conf: Config> {
	#[method(name = "bar")]
	fn method(&self) -> Result<Conf::Hash, Error>;
}

#[rpc(client, server_bounds(), client_bounds(Conf::Hash: jsonrpsee::core::DeserializeOwned))]
pub trait ServerBoundsForbidden<Conf: Config> {
	#[method(name = "bar")]
	fn method(&self) -> Result<Conf::Hash, Error>;
}

fn main() {}

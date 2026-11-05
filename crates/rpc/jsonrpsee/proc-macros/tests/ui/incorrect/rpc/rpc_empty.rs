use jsonrpsee::proc_macros::rpc;

// Empty RPC is forbidden.
#[rpc(client, server)]
pub trait Empty {}

fn main() {}

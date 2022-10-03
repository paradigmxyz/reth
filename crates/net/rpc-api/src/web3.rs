use jsonrpsee::{core::RpcResult as Result, proc_macros::rpc};
use reth_primitives::{Bytes, H256};

/// Web3 rpc interface.
#[rpc(server)]
pub trait Web3Api {
    /// Returns current client version.
    #[method(name = "web3_clientVersion")]
    fn client_version(&self) -> Result<String>;

    /// Returns sha3 of the given data.
    #[method(name = "web3_sha3")]
    fn sha3(&self, input: Bytes) -> Result<H256>;
}

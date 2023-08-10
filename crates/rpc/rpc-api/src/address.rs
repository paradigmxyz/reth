use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_primitives::Address;
use reth_rpc_types::{AddressAppearances, BlockRange};

/// address_ namespace RPC interface.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "address"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "address"))]
pub trait AddressApi {
    /// Returns transaction identifiers relevant to an address.
    ///
    /// See: <https://github.com/ethereum/execution-apis/pull/453/>
    #[method(name = "getAppearances")]
    async fn get_appearances(
        &self,
        address: Address,
        range: Option<BlockRange>,
    ) -> RpcResult<AddressAppearances>;
}

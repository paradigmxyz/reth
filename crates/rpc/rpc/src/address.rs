#![allow(dead_code, unused_variables)]
use crate::result::internal_rpc_err;
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_primitives::Address;
use reth_rpc_api::{AddressApiServer, EthApiServer};
use reth_rpc_types::{AddressAppearances, BlockRange};

/// `address` API implementation.
///
/// This type provides the functionality for handling `address` related requests.
#[derive(Debug)]
pub struct AddressApi<Eth> {
    eth: Eth,
}

impl<Eth> AddressApi<Eth> {
    /// Creates a new instance of `Address`.
    pub fn new(eth: Eth) -> Self {
        Self { eth }
    }
}

#[async_trait]
impl<Eth> AddressApiServer for AddressApi<Eth>
where
    Eth: EthApiServer,
{
    /// Handler for `address_getAppearances`
    async fn get_appearances(
        &self,
        address: Address,
        range: Option<BlockRange>,
    ) -> RpcResult<AddressAppearances> {
        Err(internal_rpc_err("unimplemented"))
    }
}

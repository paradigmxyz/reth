use async_trait::async_trait;
use reth_rpc_api::MinerApiServer;

/// `miner` API implementation.
///
/// This type provides the functionality for handling `miner` related requests.
#[derive(Clone, Default)]
pub struct MinerApi {}

#[async_trait]
impl MinerApiServer for MinerApi {}

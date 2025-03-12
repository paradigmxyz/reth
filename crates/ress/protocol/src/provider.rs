use crate::GetHeaders;
use alloy_consensus::Header;
use alloy_primitives::{Bytes, B256};
use alloy_rlp::Encodable;
use reth_ethereum_primitives::BlockBody;
use reth_network::eth_requests::{MAX_BODIES_SERVE, MAX_HEADERS_SERVE, SOFT_RESPONSE_LIMIT};
use reth_storage_errors::provider::ProviderResult;
use std::future::Future;

/// A provider trait for ress protocol.
pub trait RessProtocolProvider: Send + Sync {
    /// Return block header by hash.
    fn header(&self, block_hash: B256) -> ProviderResult<Option<Header>>;

    /// Return block headers.
    fn headers(&self, request: GetHeaders) -> ProviderResult<Vec<Header>> {
        let mut total_bytes = 0;
        let mut block_hash = request.start_hash;
        let mut headers = Vec::new();
        while let Some(header) = self.header(block_hash)? {
            block_hash = header.parent_hash;
            total_bytes += header.length();
            headers.push(header);
            if headers.len() >= request.limit as usize ||
                headers.len() >= MAX_HEADERS_SERVE ||
                total_bytes > SOFT_RESPONSE_LIMIT
            {
                break
            }
        }
        Ok(headers)
    }

    /// Return block body by hash.
    fn block_body(&self, block_hash: B256) -> ProviderResult<Option<BlockBody>>;

    /// Return block bodies.
    fn block_bodies(&self, block_hashes: Vec<B256>) -> ProviderResult<Vec<BlockBody>> {
        let mut total_bytes = 0;
        let mut bodies = Vec::new();
        for block_hash in block_hashes {
            if let Some(body) = self.block_body(block_hash)? {
                total_bytes += body.length();
                bodies.push(body);
                if bodies.len() >= MAX_BODIES_SERVE || total_bytes > SOFT_RESPONSE_LIMIT {
                    break
                }
            } else {
                break
            }
        }
        Ok(bodies)
    }

    /// Return bytecode by code hash.
    fn bytecode(&self, code_hash: B256) -> ProviderResult<Option<Bytes>>;

    /// Return witness by block hash.
    fn witness(&self, block_hash: B256) -> impl Future<Output = ProviderResult<Vec<Bytes>>> + Send;
}

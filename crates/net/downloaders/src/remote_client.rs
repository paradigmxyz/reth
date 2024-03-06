use candid::Principal;
use did::certified::CertifiedResult;
use ethereum_json_rpc_client::{reqwest::ReqwestClient, EthJsonRpcClient};
use ic_cbor::{CertificateToCbor, HashTreeToCbor};
use ic_certificate_verification::VerifyCertificate;
use ic_certification::{Certificate, HashTree, LookupResult};
use itertools::Either;
use rayon::iter::{IntoParallelIterator, ParallelIterator as _};
use reth_interfaces::p2p::{
    bodies::client::{BodiesClient, BodiesFut},
    download::DownloadClient,
    error::RequestError,
    headers::client::{HeadersClient, HeadersFut, HeadersRequest},
    priority::Priority,
};

use alloy_rlp::Decodable;
use reth_primitives::{
    BlockBody, BlockHash, BlockHashOrNumber, BlockNumber, Header, HeadersDirection, PeerId, B256,
};
use rlp::Encodable;

use std::{self, cmp::min, collections::HashMap};
use thiserror::Error;

use tracing::{debug, error, info, trace, warn};


/// Front-end API for fetching chain data from remote sources.
///
/// Blocks are assumed to have populated transactions, so reading headers will also buffer
/// transactions in memory for use in the bodies stage.

#[derive(Debug)]
pub struct RemoteClient {
    /// The buffered headers retrieved when fetching new bodies.
    headers: HashMap<BlockNumber, Header>,

    /// A mapping between block hash and number.
    hash_to_number: HashMap<BlockHash, BlockNumber>,

    /// The buffered bodies retrieved when fetching new headers.
    bodies: HashMap<BlockHash, BlockBody>,
}

/// An error that can occur when constructing and using a [`RemoteClient`].
#[derive(Debug, Error)]
pub enum RemoteClientError {
    /// An error occurred when decoding blocks, headers, or rlp headers.
    #[error(transparent)]
    Rlp(#[from] alloy_rlp::Error),

    /// An error occurred when fetching blocks, headers, or rlp headers from the remote provider.
    #[error("provider error occurred: {0}")]
    ProviderError(String),

    /// Certificate check error
    #[error("certification check error: {0}")]
    CertificateError(String),
}

/// Setting for checking last certified block
#[derive(Debug)]
pub struct CertificateCheckSettings {
    /// Principal of the EVM canister
    pub evmc_principal: String,
    /// Root key of the IC network
    pub ic_root_key: String,
}

impl RemoteClient {
    /// RemoteClient from rpc url
    pub async fn from_rpc_url(
        rpc: &str,
        start_block: u64,
        end_block: Option<u64>,
        batch_size: usize,
        certificate_settings: Option<CertificateCheckSettings>,
    ) -> Result<Self, RemoteClientError> {
        let mut headers = HashMap::new();
        let mut hash_to_number = HashMap::new();
        let mut bodies = HashMap::new();

        let reqwest_client = ethereum_json_rpc_client::reqwest::ReqwestClient::new(rpc.to_string());
        let provider = ethereum_json_rpc_client::EthJsonRpcClient::new(reqwest_client);

        let block_checker = match certificate_settings {
            None => None,
            Some(settings) => Some(BlockCertificateChecker::new(&provider, settings).await?),
        };

        const MAX_BLOCKS: u64 = 10_000;

        let last_block = provider
            .get_block_number()
            .await
            .map_err(|e| RemoteClientError::ProviderError(e.to_string()))?;

        let mut end_block = min(end_block.unwrap_or(last_block), start_block + MAX_BLOCKS);
        if let Some(block_checker) = &block_checker {
            end_block = min(end_block, block_checker.get_block_number());
        }

        info!(target: "downloaders::remote", start_block, end_block, "Fetching blocks");

        for begin_block in (start_block..=end_block).step_by(batch_size) {
            let count = std::cmp::min(batch_size as u64, end_block - begin_block);

            debug!(target: "downloaders::remote", begin_block, count, "Fetching blocks");

            let blocks_to_fetch =
                (begin_block..(begin_block + count)).map(Into::into).collect::<Vec<_>>();

            let full_blocks = provider
                .get_full_blocks_by_number(blocks_to_fetch, batch_size)
                .await
                .map_err(|e| {
                    error!(target: "downloaders::remote", begin_block, "Error fetching block: {}", e);
                    RemoteClientError::ProviderError(format!(
                        "Error fetching block {}: {}",
                        begin_block, e
                    ))
                })?
                .into_par_iter()
                .clone()
                .map(did::Block::<did::Transaction>::from)
                .collect::<Vec<_>>();

            trace!(target: "downloaders::remote", blocks = full_blocks.len(), "Fetched blocks");

            for block in full_blocks {
                if let Some(block_checker) = &block_checker {
                    block_checker.check_block(&block)?;
                }
                let header =
                    reth_primitives::Block::decode(&mut block.rlp_bytes().to_vec().as_slice())?;

                let block_hash = header.hash_slow();

                headers.insert(header.number, header.header.clone());
                hash_to_number.insert(block_hash, header.number);
                bodies.insert(
                    block_hash,
                    BlockBody {
                        transactions: header.body,
                        ommers: header.ommers,
                        withdrawals: header.withdrawals,
                    },
                );
            }
        }

        info!(blocks = headers.len(), "Initialized remote client");

        Ok(Self { headers, hash_to_number, bodies })
    }

    /// Get the remote tip hash of the chain.
    pub fn tip(&self) -> Option<B256> {
        self.headers
            .keys()
            .max()
            .and_then(|max_key| self.headers.get(max_key))
            .map(|h| h.hash_slow())
    }

    /// Returns the highest block number of this client has or `None` if empty
    pub fn max_block(&self) -> Option<u64> {
        self.headers.keys().max().copied()
    }

    /// Returns true if all blocks are canonical (no gaps)
    pub fn has_canonical_blocks(&self) -> bool {
        if self.headers.is_empty() {
            return true;
        }
        let mut nums = self.headers.keys().copied().collect::<Vec<_>>();
        nums.sort_unstable();
        let mut iter = nums.into_iter();
        let mut lowest = iter.next().expect("not empty");
        for next in iter {
            if next != lowest + 1 {
                return false;
            }
            lowest = next;
        }
        true
    }

    /// Use the provided bodies as the remote client's block body buffer.
    pub fn with_bodies(mut self, bodies: HashMap<BlockHash, BlockBody>) -> Self {
        self.bodies = bodies;
        self
    }

    /// Use the provided headers as the remote client's block body buffer.
    pub fn with_headers(mut self, headers: HashMap<BlockNumber, Header>) -> Self {
        self.headers = headers;
        for (number, header) in &self.headers {
            self.hash_to_number.insert(header.hash_slow(), *number);
        }
        self
    }
}

struct BlockCertificateChecker {
    certified_data: CertifiedResult<did::Block<did::H256>>,
    evmc_principal: Principal,
    ic_root_key: Vec<u8>
}

impl BlockCertificateChecker {
    async fn new(client: &EthJsonRpcClient<ReqwestClient>, certificate_settings: CertificateCheckSettings) -> Result<Self, RemoteClientError> {
        let evmc_principal = Principal::from_text(certificate_settings.evmc_principal).map_err(|e| RemoteClientError::CertificateError(format!("failed to parse principal: {e}")))?;
        let ic_root_key = hex::decode(&certificate_settings.ic_root_key).map_err(|e| RemoteClientError::CertificateError(format!("failed to parse IC root key: {e}")))?;
        let certified_data = client.get_last_certified_block().await.map_err(|e| RemoteClientError::ProviderError(e.to_string()))?;
        Ok(Self{certified_data: CertifiedResult{
            data: did::Block::from(certified_data.data),
            certificate: certified_data.certificate,
            witness: certified_data.witness
        }, evmc_principal, ic_root_key})
    }

    fn get_block_number(&self) -> u64 {
        self.certified_data.data.number.0.as_u64()
    }

    fn check_block(&self, block: &did::Block<did::Transaction>) -> Result<(), RemoteClientError> {
        if block.number < self.certified_data.data.number {
            return Ok(());
        }

        if block.number > self.certified_data.data.number {
            return Err(RemoteClientError::CertificateError(format!("cannot execute block {} after the latest certified", block.number)));
        }

        if block.hash != self.certified_data.data.hash {
            return Err(RemoteClientError::CertificateError(format!("state hash doesn't correspond to certified block, have {}, want {}",
                block.hash, self.certified_data.data.hash)));
        }

        let certificate = Certificate::from_cbor(&self.certified_data.certificate)
            .map_err(|e| RemoteClientError::CertificateError(format!("failed to parse certificate: {e}")))?;
        certificate.verify(self.evmc_principal.as_ref(), &self.ic_root_key)
            .map_err(|e| RemoteClientError::CertificateError(format!("certificate validation error: {e}")))?;

        let tree = HashTree::from_cbor(&self.certified_data.witness)
            .map_err(|e| RemoteClientError::CertificateError(format!("failed to parse witness: {e}")))?;
        Self::validate_tree(self.evmc_principal.as_ref(), &certificate, &tree);

        Ok(())
    }

    fn validate_tree(canister_id: &[u8], certificate: &Certificate, tree: &HashTree) -> bool {
        let certified_data_path = [
            "canister".as_bytes(),
            canister_id,
            "certified_data".as_bytes(),
        ];

        let witness = match certificate.tree.lookup_path(&certified_data_path) {
            LookupResult::Found(witness) => witness,
            _ => {
                return false;
            }
        };

        let digest = tree.digest();
        if witness != digest {
            return false;
        }

        true
    }
}

impl HeadersClient for RemoteClient {
    type Output = HeadersFut;

    fn get_headers_with_priority(
        &self,
        request: HeadersRequest,
        _priority: Priority,
    ) -> Self::Output {
        // this just searches the buffer, and fails if it can't find the header
        let mut headers = Vec::new();
        trace!(target: "downloaders::remote", request=?request, "Getting headers");

        let start_num = match request.start {
            BlockHashOrNumber::Hash(hash) => match self.hash_to_number.get(&hash) {
                Some(num) => *num,
                None => {
                    warn!(%hash, "Could not find starting block number for requested header hash");
                    return Box::pin(async move { Err(RequestError::BadResponse) });
                }
            },
            BlockHashOrNumber::Number(num) => num,
        };

        let range = if request.limit == 1 {
            Either::Left(start_num..start_num + 1)
        } else {
            match request.direction {
                HeadersDirection::Rising => Either::Left(start_num..start_num + request.limit),
                HeadersDirection::Falling => {
                    Either::Right((start_num - request.limit + 1..=start_num).rev())
                }
            }
        };

        trace!(target: "downloaders::remote", range=?range, "Getting headers with range");

        for block_number in range {
            match self.headers.get(&block_number).cloned() {
                Some(header) => headers.push(header),
                None => {
                    warn!(number=%block_number, "Could not find header");
                    return Box::pin(async move { Err(RequestError::BadResponse) });
                }
            }
        }

        Box::pin(async move { Ok((PeerId::default(), headers).into()) })
    }
}

impl BodiesClient for RemoteClient {
    type Output = BodiesFut;

    fn get_block_bodies_with_priority(
        &self,
        hashes: Vec<B256>,
        _priority: Priority,
    ) -> Self::Output {
        // this just searches the buffer, and fails if it can't find the block
        let mut bodies = Vec::new();

        // check if any are an error
        // could unwrap here
        for hash in hashes {
            match self.bodies.get(&hash).cloned() {
                Some(body) => bodies.push(body),
                None => return Box::pin(async move { Err(RequestError::BadResponse) }),
            }
        }

        Box::pin(async move { Ok((PeerId::default(), bodies).into()) })
    }
}

impl DownloadClient for RemoteClient {
    fn report_bad_message(&self, _peer_id: PeerId) {
        warn!("Reported a bad message on a remote client, the client may be corrupted or invalid");
        // noop
    }

    fn num_connected_peers(&self) -> usize {
        // no such thing as connected peers when we are just using a remote
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn remote_client_from_rpc_url() {
        let client =
            RemoteClient::from_rpc_url("https://cloudflare-eth.com", 0, Some(5), 5, None).await.unwrap();
        assert!(client.max_block().is_some());
    }

    #[tokio::test]
    async fn test_headers_client() {
        let client =
            RemoteClient::from_rpc_url("https://cloudflare-eth.com", 0, Some(5), 5, None).await.unwrap();
        let headers = client
            .get_headers_with_priority(
                HeadersRequest {
                    start: BlockHashOrNumber::Number(0),
                    limit: 5,
                    direction: HeadersDirection::Rising,
                },
                Priority::default(),
            )
            .await
            .unwrap();
        assert_eq!(headers.1.len(), 5);
    }

    #[tokio::test]
    async fn test_bodies_client() {
        let client =
            RemoteClient::from_rpc_url("https://cloudflare-eth.com", 0, Some(5), 5, None).await.unwrap();
        let headers = client
            .get_headers_with_priority(
                HeadersRequest {
                    start: BlockHashOrNumber::Number(0),
                    limit: 5,
                    direction: HeadersDirection::Rising,
                },
                Priority::default(),
            )
            .await
            .unwrap();
        let hashes = headers.1.iter().map(|h| h.hash_slow()).collect();
        let bodies =
            client.get_block_bodies_with_priority(hashes, Priority::default()).await.unwrap();
        assert_eq!(bodies.1.len(), 5);
    }
}

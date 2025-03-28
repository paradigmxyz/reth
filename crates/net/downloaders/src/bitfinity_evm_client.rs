use alloy_eips::BlockHashOrNumber;
use alloy_genesis::{ChainConfig, Genesis, GenesisAccount};
use alloy_primitives::{BlockHash, BlockNumber, B256, U256};
use candid::Principal;
use did::certified::CertifiedResult;
use ethereum_json_rpc_client::{reqwest::ReqwestClient, EthJsonRpcClient};
use eyre::Result;
use ic_cbor::{CertificateToCbor, HashTreeToCbor};
use ic_certificate_verification::VerifyCertificate;
use ic_certification::{Certificate, HashTree, LookupResult};
use itertools::Either;
use rayon::iter::{IntoParallelIterator, ParallelIterator as _};
use reth_chainspec::{
    make_genesis_header, BaseFeeParams, BaseFeeParamsKind, Chain, ChainHardforks, ChainSpec, EthereumHardfork, Hardfork
};

use alloy_rlp::Decodable;
use reth_network_p2p::{
    bodies::client::{BodiesClient, BodiesFut},
    download::DownloadClient,
    error::RequestError,
    headers::client::{HeadersClient, HeadersDirection, HeadersFut, HeadersRequest},
    priority::Priority,
};
use reth_network_peers::PeerId;
use reth_primitives::{Block, BlockBody, ForkCondition, Header, TransactionSigned};
use reth_primitives_traits::SealedHeader;
use serde_json::json;

use std::{self, collections::HashMap, time::Duration};
use thiserror::Error;

use backon::{ExponentialBuilder, Retryable};

use tracing::{debug, error, info, trace, warn};

/// RPC client configuration
#[derive(Debug, Clone)]
pub struct RpcClientConfig {
    /// Primary RPC URL
    pub primary_url: String,
    /// Backup RPC URL
    pub backup_url: Option<String>,
    /// Maximum number of retries
    pub max_retries: u32,
    /// Delay between retries
    pub retry_delay: Duration,
    /// Maximum age of the latest block to consider the EVM as active
    pub max_block_age_secs: Duration,
}

/// Front-end API for fetching chain data from remote sources.
///
/// Blocks are assumed to have populated transactions, so reading headers will also buffer
/// transactions in memory for use in the bodies stage.
#[derive(Debug)]
pub struct BitfinityEvmClient {
    /// The buffered headers retrieved when fetching new bodies.
    headers: HashMap<BlockNumber, Header>,

    /// A mapping between block hash and number.
    hash_to_number: HashMap<BlockHash, BlockNumber>,

    /// The buffered bodies retrieved when fetching new headers.
    bodies: HashMap<BlockHash, BlockBody>,

    /// The number of the last safe block.
    safe_block_number: BlockNumber,
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

    /// Error while trying to validate a block
    #[error("block validation error: {0}")]
    ValidationError(String),
}

/// Setting for checking last certified block
#[derive(Debug)]
pub struct CertificateCheckSettings {
    /// Principal of the EVM canister
    pub evmc_principal: String,
    /// Root key of the IC network
    pub ic_root_key: String,
}

impl BitfinityEvmClient {
    /// `BitfinityEvmClient` from rpc url
    pub async fn from_rpc_url(
        rpc_config: RpcClientConfig,
        start_block: u64,
        end_block: Option<u64>,
        batch_size: usize,
        max_blocks: u64,
        certificate_settings: Option<CertificateCheckSettings>,
        check_evm_state_before_importing: bool,
    ) -> Result<Self, RemoteClientError> {
        let mut headers = HashMap::new();
        let mut hash_to_number = HashMap::new();
        let mut bodies = HashMap::new();

        let provider = Self::client(rpc_config)
            .await
            .map_err(|e| RemoteClientError::ProviderError(e.to_string()))?;

        if check_evm_state_before_importing {
            match Self::is_evm_enabled(&provider).await {
                Ok(true) => {
                    info!(target: "downloaders::bitfinity_evm_client", "EVM is enabled, proceeding with import");
                }
                Ok(false) => {
                    info!(target: "downloaders::bitfinity_evm_client", "Skipping block import: EVM is disabled");
                    return Ok(Self {
                        headers,
                        hash_to_number,
                        bodies,
                        safe_block_number: BlockNumber::default(),
                    });
                }
                Err(e) => {
                    warn!(target: "downloaders::bitfinity_evm_client", "Failed to check EVM state: {}. Proceeding with import", e);
                }
            }
        }

        let block_checker = match certificate_settings {
            None => None,
            Some(settings) => Some(BlockCertificateChecker::new(&provider, settings).await?),
        };

        let latest_remote_block = provider
            .get_block_number()
            .await
            .map_err(|e| RemoteClientError::ProviderError(e.to_string()))?;

        let safe_block_number: u64 = provider
            .get_block_by_number(did::BlockNumber::Safe)
            .await
            .map_err(|e| {
                RemoteClientError::ProviderError(format!("error getting safe block: {e}"))
            })?
            .number
            .into();

        info!(target: "downloaders::bitfinity_evm_client", "Latest remote block: {latest_remote_block}");
        info!(target: "downloaders::bitfinity_evm_client", "Safe remote block number: {safe_block_number}");

        let end_block = [
            end_block.unwrap_or(u64::MAX),
            latest_remote_block,
            start_block + max_blocks - 1,
            block_checker.as_ref().map(|v| v.get_block_number()).unwrap_or(u64::MAX),
        ]
        .into_iter()
        .min()
        .unwrap_or(latest_remote_block);

        let safe_block_number =
            if safe_block_number > end_block { end_block } else { safe_block_number };

        info!(target: "downloaders::bitfinity_evm_client", "Using safe block number: {safe_block_number}");
        info!(target: "downloaders::bitfinity_evm_client", "Start fetching blocks from {} to {}", start_block, end_block);

        for begin_block in (start_block..=end_block).step_by(batch_size) {
            let count = std::cmp::min(batch_size as u64, end_block + 1 - begin_block);
            let last_block = begin_block + count - 1;

            info!(target: "downloaders::bitfinity_evm_client", "Fetching blocks from {} to {}", begin_block, last_block);

            let blocks_to_fetch = (begin_block..=last_block).map(Into::into).collect::<Vec<_>>();

            let full_blocks = provider
                .get_full_blocks_by_number(blocks_to_fetch, batch_size)
                .await
                .map_err(|e| {
                    error!(target: "downloaders::bitfinity_evm_client", begin_block, "Error fetching block: {:?}", e);
                    RemoteClientError::ProviderError(format!(
                        "Error fetching block {}: {:?}",
                        begin_block, e
                    ))
                })?
                .into_par_iter()
                .clone()
                .map(did::Block::<did::Transaction>::from)
                .collect::<Vec<_>>();

            info!(target: "downloaders::bitfinity_evm_client", blocks = full_blocks.len(), "Fetched blocks");

            for block in full_blocks {
                if let Some(block_checker) = &block_checker {
                    block_checker.check_block(&block)?;
                }
                let header =
                    reth_primitives::Header::decode(&mut block.header_rlp_encoded().as_slice())?;

                let mut body = reth_primitives::BlockBody::default();
                for tx in block.transactions {
                    let decoded_tx =
                        TransactionSigned::decode(&mut tx.rlp_encoded_2718().map_err(|e| {
                            error!(target: "downloaders::bitfinity_evm_client", begin_block, "Error encoding transaction: {:?}", e);
                            RemoteClientError::ProviderError(format!(
                                "Error fetching block {}: {:?}",
                                begin_block, e
                            ))
                        })?.to_vec().as_slice())?;
                    body.transactions.push(decoded_tx);
                }

                let block_hash = header.hash_slow();

                headers.insert(header.number, header.clone());
                hash_to_number.insert(block_hash, header.number);
                bodies.insert(block_hash, body);
            }
            info!(target: "downloaders::bitfinity_evm_client", blocks = headers.len(), "Decoded blocks");
        }

        info!(blocks = headers.len(), "Initialized remote client");

        Ok(Self { headers, hash_to_number, bodies, safe_block_number })
    }

    /// Get the remote tip hash of the chain.
    pub fn tip(&self) -> Option<B256> {
        self.headers
            .keys()
            .max()
            .and_then(|max_key| self.headers.get(max_key))
            .map(|h| h.hash_slow())
    }

    /// Returns the parent block hash of the given one.
    pub fn parent(&self, block: &B256) -> Option<B256> {
        let block_number = self.hash_to_number.get(block)?;
        if *block_number == 0 {
            return None;
        }

        self.headers.get(&(block_number - 1)).map(|h| h.hash_slow())
    }

    /// Returns unsafe blocks with transactions up to the `tip`.
    pub fn unsafe_blocks(&self, tip: &B256) -> eyre::Result<Vec<Block>> {
        let mut next_block_number = self.safe_block_number + 1;
        let mut blocks = vec![];
        while let Some(header) = self.headers.get(&next_block_number) {
            let block_hash = header.hash_slow();
            let body =
                self.bodies.get(&block_hash).ok_or_else(|| eyre::eyre!("Block body not found"))?;
            blocks.push(body.clone().into_block(header.clone()));

            if block_hash == *tip {
                return Ok(blocks);
            }

            next_block_number += 1;
        }

        Err(eyre::eyre!("Block {tip} not found"))
    }

    /// Returns the number of the given block
    pub fn get_block_number(&self, block_hash: &B256) -> Option<u64> {
        self.hash_to_number.get(block_hash).copied()
    }

    /// Returns the hash of the first block after the latest safe block. If the tip of the chain is
    /// safe, returns `None`.
    pub fn unsafe_block(&self) -> Option<B256> {
        self.headers.get(&(self.safe_block_number + 1)).map(|h| h.hash_slow())
    }

    /// Returns the block number of the last safe block in the chain.
    pub fn safe_block_number(&self) -> u64 {
        self.safe_block_number
    }

    /// Returns the hash of the last safe block in the chain.
    pub fn safe_block(&self) -> Option<B256> {
        self.headers.get(&self.safe_block_number).map(|h| h.hash_slow())
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

    /// Fetch Bitfinity chain spec
    pub async fn fetch_chain_spec(rpc: String) -> Result<ChainSpec> {
        Self::build_chain_spec(rpc).await
    }

    /// Fetch Bitfinity chain spec with fallback
    pub async fn fetch_chain_spec_with_fallback(
        primary_rpc: String,
        backup_rpc: Option<String>,
    ) -> Result<ChainSpec> {
        match Self::build_chain_spec(primary_rpc).await {
            Ok(spec) => Ok(spec),
            Err(e) => {
                warn!(target: "downloaders::bitfinity_evm_client", "Failed to fetch chain spec from primary URL: {}. Trying backup URL", e);

                if let Some(backup_rpc) = backup_rpc {
                    match Self::build_chain_spec(backup_rpc).await {
                        Ok(spec) => Ok(spec),
                        Err(e) => {
                            error!(target: "downloaders::bitfinity_evm_client", "Failed to fetch chain spec from backup URL: {}", e);

                            Err(e)
                        }
                    }
                } else {
                    error!(target: "downloaders::bitfinity_evm_client", "No backup URL provided, failed to fetch chain spec from primary URL: {}", e);

                    Err(e)
                }
            }
        }
    }

    /// Fetch Bitfinity chain spec
    async fn build_chain_spec(rpc: String) -> Result<ChainSpec> {
        let client =
            ethereum_json_rpc_client::EthJsonRpcClient::new(ReqwestClient::new(rpc.clone()));

        let chain_id = client.get_chain_id().await.map_err(|e| eyre::eyre!(e))?;

        tracing::info!("downloaders::bitfinity_evm_client - Bitfinity chain id: {}", chain_id);

        let genesis_block = client
            .get_block_by_number(0.into())
            .await
            .map_err(|e| eyre::eyre!("error getting genesis block: {}", e))?;

        let genesis_accounts =
            client.get_genesis_balances().await.map_err(|e| eyre::eyre!(e))?.into_iter().map(
                |(k, v)| {
                    tracing::info!(
                        "downloaders::bitfinity_evm_client - Bitfinity genesis account: {:?} {:?}",
                        k,
                        v
                    );
                    (k.0, GenesisAccount { balance: v.0, ..Default::default() })
                },
            );

        let chain = Chain::from_id(chain_id);

        let mut genesis: Genesis = serde_json::from_value(json!(genesis_block))
            .map_err(|e| eyre::eyre!("error parsing genesis block: {}", e))?;

        tracing::info!("downloaders::bitfinity_evm_client - Bitfinity genesis: {:?}", genesis);

        genesis.config = ChainConfig {
            chain_id,
            homestead_block: Some(0),
            eip150_block: Some(0),
            eip155_block: Some(0),
            eip158_block: Some(0),
            terminal_total_difficulty: Some(U256::ZERO),
            ..Default::default()
        };

        genesis.alloc = genesis_accounts.collect();

        let hardforks = ChainHardforks::new(vec![
            (EthereumHardfork::Frontier.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Homestead.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Dao.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Tangerine.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::SpuriousDragon.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Byzantium.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Constantinople.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Petersburg.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Istanbul.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Berlin.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::London.boxed(), ForkCondition::Block(0)),
            (
                EthereumHardfork::Paris.boxed(),
                ForkCondition::TTD {
                    activation_block_number: 0,
                    fork_block: Some(0),
                    total_difficulty: U256::from(0),
                },
            ),
        ]);

        let spec = ChainSpec {
            chain,
            genesis_header: SealedHeader::new(
                make_genesis_header(&genesis, &hardforks),
                genesis_block.hash.0,
            ),
            genesis,
            paris_block_and_final_difficulty: Some((0, U256::ZERO)),
            hardforks,
            deposit_contract: None,
            base_fee_params: BaseFeeParamsKind::Constant(BaseFeeParams::ethereum()),
            prune_delete_limit: 0,
            blob_params: Default::default(),
            bitfinity_evm_url: Some(rpc),
        };

        tracing::info!("downloaders::bitfinity_evm_client - Bitfinity chain_spec: {:?}", spec);

        Ok(spec)
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

    /// Check if the RPC endpoint is producing new blocks
    async fn is_producing_blocks(
        client: &EthJsonRpcClient<ReqwestClient>,
        max_block_age_secs: Duration,
    ) -> Result<bool> {
        debug!(target: "downloaders::bitfinity_evm_client", "Checking if the EVM is producing blocks");

        let last_block = client
            .get_block_by_number(did::BlockNumber::Latest)
            .await
            .map_err(|e| eyre::eyre!("error getting block number: {}", e))?;

        let block_ts = last_block.timestamp.0.to::<u64>();

        let current_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("Should be able to get the time")
            .as_secs();

        Ok(current_ts - block_ts <= max_block_age_secs.as_secs())
    }

    /// Creates a new JSON-RPC client with retry functionality
    ///
    /// Tries the primary URL first, falls back to backup URL if provided or if primary is not
    /// producing blocks
    pub async fn client(config: RpcClientConfig) -> Result<EthJsonRpcClient<ReqwestClient>> {
        // Create retry configuration
        let backoff = ExponentialBuilder::default()
            .with_max_times(config.max_retries as _)
            .with_max_delay(config.retry_delay)
            .with_jitter();

        // Try primary URL first
        if let Ok(primary) = Self::connect_and_verify(&config.primary_url, &backoff).await {
            if Self::is_producing_blocks(&primary, config.max_block_age_secs).await? {
                debug!(target: "downloaders::bitfinity_evm_client", "Using primary RPC endpoint - producing blocks");

                return Ok(primary);
            }
        }

        // Try backup URL if available
        if let Some(backup_url) = config.backup_url {
            if let Ok(backup) = Self::connect_and_verify(&backup_url, &backoff).await {
                if Self::is_producing_blocks(&backup, config.max_block_age_secs).await? {
                    warn!(target: "downloaders::bitfinity_evm_client", "Primary RPC endpoint not producing blocks, using backup");

                    return Ok(backup);
                }
            }
        }

        // Fall back to primary if it connected but wasn't producing blocks
        if let Ok(primary) = Self::connect_and_verify(&config.primary_url, &backoff).await {
            warn!(target: "downloaders::bitfinity_evm_client", "No endpoints producing blocks, using primary as fallback");

            return Ok(primary);
        }

        Err(eyre::eyre!("Failed to connect to any RPC endpoint"))
    }

    /// Helper function to connect to an RPC endpoint and verify the connection
    async fn connect_and_verify(
        url: &str,
        backoff: &ExponentialBuilder,
    ) -> Result<EthJsonRpcClient<ReqwestClient>> {
        let client = EthJsonRpcClient::new(ReqwestClient::new(url.to_string()));

        // Test connection by getting chain ID
        Ok((|| async { client.get_chain_id().await })
            .retry(backoff)
            .notify(|e, dur| {
                error!(
                    target: "downloaders::bitfinity_evm_client",
                    "Failed to connect to {} after {}s: {}",
                    url,
                    dur.as_secs(),
                    e
                );
            })
            .await
            .map(|_| client)
            .map_err(|e| RemoteClientError::ProviderError(e.to_string()))?)
    }

    /// Check if the node is enabled
    pub async fn is_evm_enabled(client: &EthJsonRpcClient<ReqwestClient>) -> Result<bool> {
        let state = client.get_evm_global_state().await.map_err(|e| {
            RemoteClientError::ProviderError(format!("failed to get evm global state: {e}"))
        })?;

        Ok(state.is_enabled())
    }
}

struct BlockCertificateChecker {
    certified_data: CertifiedResult<did::Block<did::H256>>,
    evmc_principal: Principal,
    ic_root_key: Vec<u8>,
}

impl BlockCertificateChecker {
    async fn new(
        client: &EthJsonRpcClient<ReqwestClient>,
        certificate_settings: CertificateCheckSettings,
    ) -> Result<Self, RemoteClientError> {
        let evmc_principal =
            Principal::from_text(certificate_settings.evmc_principal).map_err(|e| {
                RemoteClientError::CertificateError(format!("failed to parse principal: {e}"))
            })?;
        let ic_root_key = hex::decode(&certificate_settings.ic_root_key).map_err(|e| {
            RemoteClientError::CertificateError(format!("failed to parse IC root key: {e}"))
        })?;
        let certified_data = client
            .get_last_certified_block()
            .await
            .map_err(|e| RemoteClientError::ProviderError(e.to_string()))?;
        Ok(Self { certified_data, evmc_principal, ic_root_key })
    }

    fn get_block_number(&self) -> u64 {
        self.certified_data.data.number.as_u64()
    }

    fn check_block(&self, block: &did::Block<did::Transaction>) -> Result<(), RemoteClientError> {
        if block.number < self.certified_data.data.number {
            return Ok(());
        }

        if block.number > self.certified_data.data.number {
            return Err(RemoteClientError::CertificateError(format!(
                "cannot execute block {} after the latest certified",
                block.number
            )));
        }

        if block.hash != self.certified_data.data.hash {
            return Err(RemoteClientError::CertificateError(format!(
                "state hash doesn't correspond to certified block, have {}, want {}",
                block.hash, self.certified_data.data.hash
            )));
        }

        let certificate =
            Certificate::from_cbor(&self.certified_data.certificate).map_err(|e| {
                RemoteClientError::CertificateError(format!("failed to parse certificate: {e}"))
            })?;

        let current_time_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("Should be able to get the time")
            .as_nanos();
        let allowed_certificate_time_offset = Duration::from_secs(120).as_nanos();

        certificate
            .verify(
                self.evmc_principal.as_ref(),
                &self.ic_root_key,
                &current_time_ns,
                &allowed_certificate_time_offset,
            )
            .map_err(|e| {
                RemoteClientError::CertificateError(format!("certificate validation error: {e}"))
            })?;

        let tree = HashTree::from_cbor(&self.certified_data.witness).map_err(|e| {
            RemoteClientError::CertificateError(format!("failed to parse witness: {e}"))
        })?;
        Self::validate_tree(self.evmc_principal.as_ref(), &certificate, &tree);

        Ok(())
    }

    fn validate_tree(canister_id: &[u8], certificate: &Certificate, tree: &HashTree) -> bool {
        let certified_data_path = [b"canister", canister_id, b"certified_data"];

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

impl HeadersClient for BitfinityEvmClient {
    type Output = HeadersFut;
    type Header = Header;

    fn get_headers_with_priority(
        &self,
        request: HeadersRequest,
        _priority: Priority,
    ) -> Self::Output {
        // this just searches the buffer, and fails if it can't find the header
        let mut headers = Vec::new();
        trace!(target: "downloaders::bitfinity_evm_client", request=?request, "Getting headers");

        let start_num = match request.start {
            BlockHashOrNumber::Hash(hash) => match self.hash_to_number.get(&hash) {
                Some(num) => *num,
                None => {
                    error!(%hash, "Could not find starting block number for requested header hash");
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

        trace!(target: "downloaders::bitfinity_evm_client", range=?range, "Getting headers with range");

        for block_number in range {
            match self.headers.get(&block_number).cloned() {
                Some(header) => headers.push(header),
                None => {
                    error!(number=%block_number, "Could not find header");
                    return Box::pin(async move { Err(RequestError::BadResponse) });
                }
            }
        }

        Box::pin(async move { Ok((PeerId::default(), headers).into()) })
    }
}

impl BodiesClient for BitfinityEvmClient {
    type Output = BodiesFut;
    type Body = BlockBody;

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
                None => {
                    error!(%hash, "Could not find body for requested block hash");
                    return Box::pin(async move { Err(RequestError::BadResponse) });
                }
            }
        }

        Box::pin(async move { Ok((PeerId::default(), bodies).into()) })
    }
}

impl DownloadClient for BitfinityEvmClient {
    fn report_bad_message(&self, _peer_id: PeerId) {
        error!("Reported a bad message on a remote client, the client may be corrupted or invalid");
        // Uncomment this panic to stop the import job and print the stacktrace of the error
        // panic!("Reported a bad message on a remote client, the client may be corrupted or
        // invalid");
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
    async fn bitfinity_remote_client_from_rpc_url() {
        let client = BitfinityEvmClient::from_rpc_url(
            RpcClientConfig {
                primary_url: "https://cloudflare-eth.com".to_string(),
                backup_url: None,
                max_retries: 3,
                retry_delay: Duration::from_secs(1),
                max_block_age_secs: Duration::from_secs(600),
            },
            0,
            Some(5),
            5,
            1000,
            None,
            false,
        )
        .await
        .unwrap();
        assert!(client.max_block().is_some());
    }

    #[tokio::test]
    async fn bitfinity_remote_client_from_backup_rpc_url() {
        let client = BitfinityEvmClient::from_rpc_url(
            RpcClientConfig {
                primary_url: "https://cloudflare.com".to_string(),
                backup_url: Some("https://cloudflare-eth.com".to_string()),
                max_retries: 3,
                retry_delay: Duration::from_secs(1),
                max_block_age_secs: Duration::from_secs(600),
            },
            0,
            Some(5),
            5,
            1000,
            None,
            false,
        )
        .await
        .unwrap();
        assert!(client.max_block().is_some());
    }

    #[tokio::test]
    async fn bitfinity_test_headers_client() {
        let client = BitfinityEvmClient::from_rpc_url(
            RpcClientConfig {
                primary_url: "https://cloudflare-eth.com".to_string(),
                backup_url: None,
                max_retries: 3,
                retry_delay: Duration::from_secs(1),
                max_block_age_secs: Duration::from_secs(600),
            },
            0,
            Some(5),
            5,
            1000,
            None,
            false,
        )
        .await
        .unwrap();
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
    async fn bitfinity_test_bodies_client() {
        let client = BitfinityEvmClient::from_rpc_url(
            RpcClientConfig {
                primary_url: "https://cloudflare-eth.com".to_string(),
                backup_url: None,
                max_retries: 3,
                retry_delay: Duration::from_secs(1),
                max_block_age_secs: Duration::from_secs(600),
            },
            0,
            Some(5),
            5,
            1000,
            None,
            false,
        )
        .await
        .unwrap();
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

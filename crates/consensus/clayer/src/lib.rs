mod consensus;
mod engine_api;
mod engine_pbft;
mod error;
mod task;
mod timing;
use crate::engine_api::{
    auth::{Auth, JwtKey},
    http::HttpJsonRpc,
};
pub use consensus::{ClayerConsensusEngine, ClayerConsensusMessagingAgent};
use consensus::{PbftConfig, PbftState};
use engine_api::http_blocking::HttpJsonRpcSync;
pub use engine_api::AuthHttpConfig;

use reth_network::NetworkHandle;
use reth_primitives::{ChainSpec, SealedHeader};
use reth_provider::{BlockReaderIdExt, ConsensusNumberReader, ConsensusNumberWriter};

use secp256k1::SecretKey;
use std::sync::Arc;
use task::ClTask;

use url::Url;

pub fn create_api(config: &AuthHttpConfig) -> HttpJsonRpc {
    let str = format!("http://127.0.0.1:{}/", config.port);
    let execution_url = Url::parse(&str).unwrap();
    let execution_timeout_multiplier = Option::from(3);

    let jwt_key = JwtKey::from_slice(&config.auth).unwrap();

    let auth = Auth::new(jwt_key, None, None);
    let api = match HttpJsonRpc::new_with_auth(execution_url, auth, execution_timeout_multiplier) {
        Ok(api) => api,
        Err(e) => {
            panic!("Failed to create execution api. Error: {:?}", e);
        }
    };
    api
}

pub fn create_sync_api(config: &AuthHttpConfig) -> HttpJsonRpcSync {
    let str = format!("http://127.0.0.1:{}/", config.port);
    let execution_url = Url::parse(&str).unwrap();
    let execution_timeout_multiplier = Option::from(3);

    let jwt_key = JwtKey::from_slice(&config.auth).unwrap();

    let auth = Auth::new(jwt_key, None, None);
    let api =
        match HttpJsonRpcSync::new_with_auth(execution_url, auth, execution_timeout_multiplier) {
            Ok(api) => api,
            Err(e) => {
                panic!("Failed to create execution api. Error: {:?}", e);
            }
        };
    api
}

pub struct ConsensusBuilder<Client, CDB> {
    chain_spec: Arc<ChainSpec>,
    client: Client,
    network: NetworkHandle,
    consensus_agent: ClayerConsensusMessagingAgent,
    storages: CDB,
    config: PbftConfig,
    state: PbftState,
    latest_header: SealedHeader,
    auth_config: AuthHttpConfig,
}

impl<Client, CDB> ConsensusBuilder<Client, CDB>
where
    Client: BlockReaderIdExt,
{
    /// Creates a new builder instance to configure all parts.
    pub fn new(
        secret: SecretKey,
        chain_spec: Arc<ChainSpec>,
        client: Client,
        network: NetworkHandle,
        clayer_consensus_messaging_agent: ClayerConsensusMessagingAgent,
        storages: CDB,
        auth_config: AuthHttpConfig,
        validators_dir: std::path::PathBuf,
    ) -> Self {
        let latest_header = client
            .latest_header()
            .ok()
            .flatten()
            .unwrap_or_else(|| chain_spec.sealed_genesis_header());

        let config = PbftConfig::new(validators_dir);
        let state = PbftState::new(secret, latest_header.number, latest_header.timestamp, &config);
        Self {
            chain_spec,
            client,
            network,
            consensus_agent: clayer_consensus_messaging_agent,
            storages,
            config,
            state,
            latest_header,
            auth_config,
        }
    }
    /// Consumes the type and returns all components
    #[track_caller]
    pub fn build(self) -> ClTask<Client, CDB>
    where
        CDB: ConsensusNumberReader + ConsensusNumberWriter + 'static,
        Client: BlockReaderIdExt + Clone + 'static,
    {
        let Self {
            chain_spec,
            client,

            network,
            consensus_agent,
            storages,
            config,
            state,
            latest_header,
            auth_config,
        } = self;
        let task = ClTask::new(
            Arc::clone(&chain_spec),
            client,
            auth_config,
            network.clone(),
            consensus_agent,
            storages,
            config,
            state,
            latest_header,
        );
        task
    }
}

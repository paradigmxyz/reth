use config::{Config, File};
use reth_rpc_types::PeerId;
use serde::Deserialize;
use std::{path::PathBuf, str::FromStr, time::Duration};

#[derive(Debug, Clone)]
pub struct PbftConfig {
    /// Members of the PBFT network
    pub members: Vec<PeerId>,

    /// Minimum time between publishing blocks
    pub block_publishing_min_interval: Duration,

    /// How long to wait in between trying to publish blocks
    pub block_publishing_delay: Duration,

    /// How long to wait for an update to arrive from the validator
    pub update_recv_timeout: Duration,

    /// The base time to use for retrying with exponential backoff
    pub exponential_retry_base: Duration,

    /// The maximum time for retrying with exponential backoff
    pub exponential_retry_max: Duration,

    /// How long to wait for the next BlockNew + PrePrepare before determining primary is faulty
    /// Must be longer than block_publishing_delay
    pub idle_timeout: Duration,

    /// How long to wait (after Pre-Preparing) for the node to commit the block before starting a
    /// view change (guarantees liveness by allowing the network to get "unstuck" if it is unable
    /// to commit a block)
    pub commit_timeout: Duration,

    /// When view changing, how long to wait for a valid NewView message before starting a
    /// different view change
    pub view_change_duration: Duration,

    /// How many blocks to commit before forcing a view change for fairness
    pub forced_view_change_interval: u64,

    /// How large the PbftLog is allowed to get before being pruned
    pub max_log_size: u64,
}

impl Default for PbftConfig {
    fn default() -> Self {
        PbftConfig {
            members: Vec::new(),
            block_publishing_min_interval: Duration::from_millis(5000),
            block_publishing_delay: Duration::from_millis(1000),
            update_recv_timeout: Duration::from_millis(10),
            exponential_retry_base: Duration::from_millis(100),
            exponential_retry_max: Duration::from_millis(60000),
            idle_timeout: Duration::from_millis(30000),
            commit_timeout: Duration::from_millis(10000),
            view_change_duration: Duration::from_millis(5000),
            forced_view_change_interval: 20,
            max_log_size: 10000,
        }
    }
}

impl PbftConfig {
    pub fn new(path: PathBuf) -> Self {
        Self { members: load_members_config(path), ..Default::default() }
    }
}

pub fn load_members_config(path: PathBuf) -> Vec<PeerId> {
    #[derive(Debug, Deserialize, Clone)]
    pub struct ValidatorsConfig {
        pub validators: Vec<String>,
    }
    let mut conf = Config::default();
    conf.merge(File::from(path)).expect("members config file error");
    let config: ValidatorsConfig = conf.try_into().expect("try into ValidatorsConfig error");

    let mut members = Vec::new();
    for s in config.validators.iter() {
        let id = PeerId::from_str(s).expect("try into PeerId error");
        members.push(id);
    }
    members
}

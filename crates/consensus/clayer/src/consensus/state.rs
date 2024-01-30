use super::{config::PbftConfig, pbft_error::PbftError, validators::Validators};
use crate::timing::Timeout;
use alloy_primitives::{Address, Bytes, B256};
use reth_ecies::util::pk2id;
use reth_network::config::SecretKey;
use reth_primitives::public_key_to_address;
use reth_rpc_types::PeerId;
use secp256k1::{KeyPair, PublicKey, SECP256K1};
use serde_derive::{Deserialize, Serialize};
use std::{fmt, time::Duration};
use tracing::{debug, info};

pub const PRIMARY_FIXED: bool = true;

/// Phases of the PBFT algorithm, in `Normal` mode
#[derive(Debug, PartialEq, Eq, PartialOrd, Clone, Serialize, Deserialize)]
pub enum PbftPhase {
    PrePreparing,
    Preparing,
    Committing,
    // Node is waiting for a BlockCommit (bool indicates if it's a catch-up commit)
    Finishing(bool),
}
impl fmt::Display for PbftPhase {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                PbftPhase::PrePreparing => "PrePreparing".into(),
                PbftPhase::Preparing => "Preparing".into(),
                PbftPhase::Committing => "Committing".into(),
                PbftPhase::Finishing(cu) => format!("Finishing {}", cu),
            },
        )
    }
}

/// Modes that the PBFT algorithm can possibly be in
#[derive(Debug, PartialEq, Eq, Copy, Clone, Serialize, Deserialize)]
pub enum PbftMode {
    Normal,
    /// Contains the view number of the view this node is attempting to change to
    ViewChanging(u64),
}

/// Information about the PBFT algorithm's state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PbftState {
    /// This node's ID
    pub id: PeerId,

    /// This node's KeyPair
    pub kp: KeyPair,

    /// The node's current sequence number
    pub seq_num: u64,

    /// The current view
    pub view: u64,

    /// The block ID of the node's current chain head
    pub chain_head: B256,

    /// Current phase of the algorithm
    pub phase: PbftPhase,

    /// Normal operation or view changing
    pub mode: PbftMode,

    /// List of validators
    pub validators: Validators,

    /// The maximum number of faulty nodes in the network
    pub f: u64,

    pub block_publishing_timeout: Timeout,

    /// Timer used to make sure the primary publishes blocks in a timely manner. If not, then this
    /// node will initiate a view change.
    pub idle_timeout: Timeout,

    /// Timer used to make sure the network doesn't get stuck if it fails to commit a block in a
    /// reasonable amount of time. If it doesn't commit a block in time, this node will initiate a
    /// view change when the timer expires.
    pub commit_timeout: Timeout,

    /// When view changing, timer is used to make sure a valid NewView message is sent by the new
    /// primary in a timely manner. If not, this node will start a different view change.
    pub view_change_timeout: Timeout,

    /// The duration of the view change timeout; when a view change is initiated for view v + 1,
    /// the timeout will be equal to the `view_change_duration`; if the timeout expires and the
    /// node starts a change to view v + 2, the timeout will be `2 * view_change_duration`; etc.
    pub view_change_duration: Duration,

    /// The base time to use for retrying with exponential backoff
    pub exponential_retry_base: Duration,

    /// The maximum time for retrying with exponential backoff
    pub exponential_retry_max: Duration,

    /// How many blocks to commit before forcing a view change for fairness
    pub forced_view_change_interval: u64,
}

impl fmt::Display for PbftState {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let is_primary = if self.is_primary() { " *" } else { "" };
        let phase = if let PbftMode::ViewChanging(v) = self.mode {
            format!("ViewChanging({})", v)
        } else {
            match self.phase {
                PbftPhase::PrePreparing => "PrePreparing".into(),
                PbftPhase::Preparing => "Preparing".into(),
                PbftPhase::Committing => "Committing".into(),
                PbftPhase::Finishing(cu) => format!("Finishing({})", cu),
            }
        };
        write!(f, "({}, view {}, seq {}{})", phase, self.view, self.seq_num, is_primary)
    }
}

impl PbftState {
    pub fn new(sk: SecretKey, head_block_num: u64, config: &PbftConfig) -> Self {
        let kp = KeyPair::from_secret_key(SECP256K1, &sk);
        let id = pk2id(&kp.public_key());
        // Maximum number of faulty nodes in this network. Panic if there are not enough nodes.
        let f = ((config.members.len() - 1) / 3) as u64;
        if f == 0 {
            panic!("This network does not contain enough nodes to be fault tolerant");
        }

        PbftState {
            id,
            kp,
            seq_num: head_block_num + 1,
            view: 0,
            chain_head: B256::default(),
            phase: PbftPhase::PrePreparing,
            mode: PbftMode::Normal,
            f,
            validators: Validators::new(config.members.clone()),
            block_publishing_timeout: Timeout::new(config.block_publishing_delay),
            idle_timeout: Timeout::new(config.idle_timeout),
            commit_timeout: Timeout::new(config.commit_timeout),
            view_change_timeout: Timeout::new(config.view_change_duration),
            view_change_duration: config.view_change_duration,
            exponential_retry_base: config.exponential_retry_base,
            exponential_retry_max: config.exponential_retry_max,
            forced_view_change_interval: config.forced_view_change_interval,
        }
    }
    /// Obtain the ID for the primary node in the network
    pub fn get_primary_id(&self) -> PeerId {
        let mut view = self.view;
        if PRIMARY_FIXED {
            view = 0;
        }
        self.validators.get_primary_id(view)
    }

    pub fn get_primary_id_at_view(&self, view: u64) -> PeerId {
        let mut view = view;
        if PRIMARY_FIXED {
            view = 0;
        }
        let primary_index = (view as usize) % self.validators.len();
        self.validators.index(primary_index).clone()
    }

    /// Tell if this node is currently the primary
    pub fn is_primary(&self) -> bool {
        let mut view = self.view;
        if PRIMARY_FIXED {
            view = 0;
        }
        self.validators.is_primary(self.id.clone(), view)
    }

    /// Tell if this node is the primary at the specified view
    pub fn is_primary_at_view(&self, view: u64) -> bool {
        let mut view = view;
        if PRIMARY_FIXED {
            view = 0;
        }
        self.id == self.get_primary_id_at_view(view)
    }

    /// Switch to the desired phase if it is the next phase of the algorithm; if it is not the next
    /// phase, return an error
    pub fn switch_phase(&mut self, desired_phase: PbftPhase) -> Result<(), PbftError> {
        let is_next_phase = {
            if let PbftPhase::Finishing(_) = desired_phase {
                self.phase == PbftPhase::Committing
            } else {
                desired_phase
                    == match self.phase {
                        PbftPhase::PrePreparing => PbftPhase::Preparing,
                        PbftPhase::Preparing => PbftPhase::Committing,
                        PbftPhase::Finishing(_) => PbftPhase::PrePreparing,
                        _ => panic!("All conditions should be accounted for already"),
                    }
            }
        };
        if is_next_phase {
            debug!(target:"consensus::cl","{}: Changing to {}", self, desired_phase);
            self.phase = desired_phase;
            Ok(())
        } else {
            Err(PbftError::InternalError(format!(
                "Node is in {} phase; attempted to switch to {}",
                self.phase, desired_phase
            )))
        }
    }

    pub fn at_forced_view_change(&self) -> bool {
        self.seq_num % self.forced_view_change_interval == 0
    }
}

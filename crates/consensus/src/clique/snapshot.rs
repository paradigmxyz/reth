//! Implementation of clique voting snapshots.
//! https://github.com/ethereum/go-ethereum/blob/d0a4989a8def7e6bad182d1513e8d4a093c1672d/consensus/clique/snapshot.go

use std::collections::{hash_map::Entry, HashMap, HashSet};

use reth_interfaces::consensus::CliqueError;
use reth_primitives::{Address, BlockHash, BlockNumber, CliqueConfig, SealedHeader, H64};

use super::{
    constants::{NONCE_AUTH_VOTE, NONCE_DROP_VOTE},
    utils::recover_header_signer,
};

/// Vote represents a single vote that an authorized signer made to modify the
/// list of authorizations.
#[derive(Debug, Clone)]
pub struct Vote {
    /// Authorized signer that cast this vote
    signer: Address,
    /// Block number the vote was cast in (expire old votes)
    block: BlockNumber,
    /// Account being voted on to change its authorization
    address: Address,
    /// Whether to authorize or deauthorize the voted account
    authorize: bool,
}

impl Vote {
    /// Create a new vote.
    pub fn new(header: &SealedHeader, signer: Address, authorize: bool) -> Self {
        Self { address: header.beneficiary, block: header.number, authorize, signer }
    }
}

/// Tally is a simple vote tally to keep the current score of votes. Votes that
/// go against the proposal aren't counted since it's equivalent to not voting.
#[derive(Debug, Clone)]
pub struct Tally {
    /// Whether the vote is about authorizing or kicking someone
    authorize: bool,
    /// Number of votes until now wanting to pass the proposal
    votes: u64,
}

impl Tally {
    pub fn new(authorize: bool) -> Self {
        Self { authorize, votes: 0 }
    }

    pub fn upvote(&mut self) {
        self.votes += 1;
    }

    pub fn downvote(&mut self) {
        self.votes = self.votes.saturating_sub(1);
    }

    pub fn is_empty(&self) -> bool {
        self.votes == 0
    }
}

/// Snapshot is the state of the authorization voting at a given point in time.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Block number where the snapshot was created
    number: BlockNumber,
    /// Block hash where the snapshot was created
    hash: BlockHash,
    /// Set of authorized signers at this moment
    signers: HashSet<Address>,
    // Set of recent signers for spam protections
    // Recents map[uint64]common.Address
    /// List of votes cast in chronological order
    votes: Vec<Vote>,
    /// Current vote tally to avoid recalculating
    tally: HashMap<Address, Tally>,
    /// The clique configuration.
    config: CliqueConfig,
}

impl Snapshot {
    /// Check whether it makes sense to cast the specified vote in the
    /// given snapshot context (e.g. don't try to add an already authorized signer).
    pub fn is_valid_vote(&self, vote: &Vote) -> bool {
        let signer_is_authorized = self.is_authorized(&vote.address);
        (signer_is_authorized && !vote.authorize) || (!signer_is_authorized && vote.authorize)
    }

    /// Check if the address is an authorized signer.
    pub fn is_authorized(&self, signer: &Address) -> bool {
        self.signers.contains(signer)
    }

    /// Add a new vote into the tally.
    pub fn cast(&mut self, vote: Vote) {
        // Check if the vote is valid, otherwise discard.
        if self.is_valid_vote(&vote) {
            // TODO: what if `authorize` flag doesn't match?
            self.tally.entry(vote.address).or_insert(Tally::new(vote.authorize)).upvote();
            self.votes.push(vote);
        }
    }

    /// Remove a previously cast vote from the tally.
    pub fn uncast(&mut self, vote: &Vote) -> bool {
        match self.tally.entry(vote.address) {
            Entry::Occupied(mut entry) if entry.get().authorize == vote.authorize => {
                entry.get_mut().downvote();
                if entry.get().is_empty() {
                    entry.remove();
                }
                true
            }
            _ => false,
        }
    }

    /// Clear votes and tallies. Must be called on checkpoint blocks.
    pub fn clear(&mut self) {
        self.votes.clear();
        self.tally.clear();
    }

    /// Create a new authorization snapshot by applying the given header to
    /// the original one.
    pub fn apply(&self, header: &SealedHeader) -> Result<Self, CliqueError> {
        let mut next_snapshot = self.clone();

        let expected = self.number + 1;
        if header.number != expected {
            return Err(CliqueError::InvalidVotingChain { expected, received: header.number })
        }

        // TODO: Delete the oldest signer from the recent list to allow it signing again

        let is_checkpoint = header.number % self.config.epoch == 0;
        if is_checkpoint {
            next_snapshot.clear();
        }

        // TODO: validate the signer
        let signer = recover_header_signer(header)?;
        if !self.is_authorized(&signer) {
            return Err(CliqueError::UnauthorizedSigner { signer })
        }

        // TODO: store and check recents

        // TODO: Header authorized, discard any previous votes from the signer

        // TODO: Tally up the new vote from the signer
        let authorize = match H64::from_low_u64_ne(header.nonce).to_fixed_bytes() {
            NONCE_AUTH_VOTE => true,
            NONCE_DROP_VOTE => false,
            _ => return Err(CliqueError::InvalidVote { nonce: header.nonce }),
        };
        let vote = Vote::new(header, signer, authorize);
        next_snapshot.cast(vote);

        // TODO: If the vote passed, update the list of signers

        next_snapshot.number = header.number;
        next_snapshot.hash = header.hash();
        Ok(next_snapshot)
    }
}

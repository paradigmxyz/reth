//! Implementation of clique voting snapshots.
//! https://github.com/ethereum/go-ethereum/blob/d0a4989a8def7e6bad182d1513e8d4a093c1672d/consensus/clique/snapshot.go

use reth_interfaces::consensus::CliqueError;
use reth_primitives::{Address, BlockHash, BlockNumber, CliqueConfig, SealedHeader, H64};
use std::collections::{hash_map::Entry, BTreeSet, HashMap};

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
    fn new(header: &SealedHeader, signer: Address) -> Result<Self, CliqueError> {
        let authorize = match H64::from_low_u64_ne(header.nonce).to_fixed_bytes() {
            NONCE_AUTH_VOTE => true,
            NONCE_DROP_VOTE => false,
            _ => return Err(CliqueError::InvalidVote { nonce: header.nonce }),
        };
        Ok(Self { address: header.beneficiary, block: header.number, authorize, signer })
    }

    /// Check if the other vote is the same
    fn is_same_vote(&self, other: &Vote) -> bool {
        self.signer == other.signer && self.address == other.address
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
    signers: BTreeSet<Address>, // TODO: make sure this is ordered asc
    // Set of recent signers for spam protections
    recents: HashMap<BlockNumber, Address>,
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
        let signer_is_authorized = self.is_authorized_signer(&vote.address);
        (signer_is_authorized && !vote.authorize) || (!signer_is_authorized && vote.authorize)
    }

    /// Check if the address is an authorized signer.
    pub fn is_authorized_signer(&self, signer: &Address) -> bool {
        self.signers.contains(signer)
    }

    /// Check if the address is a recent signer.
    pub fn is_recent_signer(&self, signer: &Address) -> bool {
        self.recents.iter().any(|(_, s)| s == signer)
    }

    /// Return true if the vote has passed.
    pub fn vote_has_passed(&self, tally: &Tally) -> bool {
        let pass_threshold = self.signers.len() / 2;
        tally.votes > pass_threshold as u64
    }

    /// Return a flag if a signer at a given block height is in-turn or not.
    pub fn is_signer_inturn(&self, signer: &Address, block: BlockNumber) -> Option<bool> {
        self.signers
            .iter()
            .position(|s| s == signer)
            .map(|idx| block % self.signers.len() as u64 == idx as u64)
    }

    /// Add a new vote into the cached tally and ordered collection of votes.
    /// Remove a vote if there was one already.
    fn cast(&mut self, vote: Vote) {
        // Discard any previous votes from the signer.
        // Only one vote allowed per signer.
        if let Some(idx) = self.votes.iter().position(|v| v.is_same_vote(&vote)) {
            let removed = self.votes.remove(idx);
            self.uncast(&removed);
        }

        // Check if the vote is valid, otherwise discard.
        if self.is_valid_vote(&vote) {
            // TODO: what if `authorize` flag doesn't match?
            self.tally.entry(vote.address).or_insert(Tally::new(vote.authorize)).upvote();
            self.votes.push(vote);
        }
    }

    /// Remove a previously cast vote from the tally and ordered collection of votes.
    fn uncast(&mut self, vote: &Vote) {
        // Uncast the votes from the cached tally
        if let Entry::Occupied(mut entry) = self.tally.entry(vote.address) {
            // Ensure we only revert counted votes
            if entry.get().authorize == vote.authorize {
                entry.get_mut().downvote();

                // Revert the vote if no votes left.
                if entry.get().is_empty() {
                    entry.remove();
                }
            }
        }
    }

    /// Shrink the list of recent signers if above the threshold.
    fn shrink_recents_cache(&mut self, block: BlockNumber) {
        let recents_threshold = self.signers.len() as u64 / 2 + 1;
        if block >= recents_threshold {
            let at_block_number = block - recents_threshold;
            self.recents.remove(&at_block_number);
        }
    }

    /// Clear votes and tallies. Must be called on checkpoint blocks.
    fn clear(&mut self) {
        self.votes.clear();
        self.tally.clear();
    }

    /// Create a new authorization snapshot by applying the given header to
    /// the original one.
    pub fn apply(&mut self, header: &SealedHeader) -> Result<(), CliqueError> {
        let expected = self.number + 1;
        if header.number != expected {
            return Err(CliqueError::InvalidVotingChain { expected, received: header.number })
        }

        // Remove any votes on checkpoint blocks
        let is_checkpoint = header.number % self.config.epoch == 0;
        if is_checkpoint {
            self.clear();
        }

        // Delete the oldest signer from the recent list to allow it signing again
        self.shrink_recents_cache(header.number);

        // Resolve the authorization key and check against signers
        let signer = recover_header_signer(header)?;
        if !self.is_authorized_signer(&signer) {
            return Err(CliqueError::UnauthorizedSigner { signer })
        }
        if self.is_recent_signer(&signer) {
            return Err(CliqueError::RecentSigner { signer })
        }
        self.recents.insert(header.number, signer);

        // Tally up the new vote from the signer.
        let vote = Vote::new(header, signer)?;
        self.cast(vote);

        // If the vote passed, update the list of signers
        let vote_subject = header.beneficiary;
        if let Some(tally) = self.tally.get(&vote_subject).filter(|t| self.vote_has_passed(t)) {
            if tally.authorize {
                self.signers.insert(vote_subject);
            } else {
                self.signers.remove(&vote_subject);

                // Signer list shrunk, delete any leftover recent caches
                self.shrink_recents_cache(header.number);

                // Discard any previous votes the deauthorized signer cast
                // `drain_filter` is still unstable (https://doc.rust-lang.org/std/vec/struct.Vec.html#method.drain_filter)
                let mut votes_to_discard = Vec::default();
                self.votes.retain(|v| {
                    if v.signer == vote_subject {
                        votes_to_discard.push(v.clone());
                        false
                    } else {
                        true
                    }
                });
                votes_to_discard.into_iter().for_each(|v| self.uncast(&v));
            }

            // Discard any previous votes around the just changed account
            self.votes.retain(|v| v.address != vote_subject);
            self.tally.remove(&vote_subject);
        }

        self.number = header.number;
        self.hash = header.hash();
        Ok(())
    }
}

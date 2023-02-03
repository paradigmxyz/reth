//! Implementation of clique voting snapshots.
//! https://github.com/ethereum/go-ethereum/blob/d0a4989a8def7e6bad182d1513e8d4a093c1672d/consensus/clique/snapshot.go

use std::collections::{HashMap, HashSet};

use reth_primitives::{Address, BlockHash, BlockNumber};

/// Vote represents a single vote that an authorized signer made to modify the
/// list of authorizations.
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

/// Tally is a simple vote tally to keep the current score of votes. Votes that
/// go against the proposal aren't counted since it's equivalent to not voting.
pub struct Tally {
    /// Whether the vote is about authorizing or kicking someone
    authorize: bool,
    /// Number of votes until now wanting to pass the proposal
    votes: u64,
}

/// Snapshot is the state of the authorization voting at a given point in time.
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
}

impl Snapshot {
    /// Check whether it makes sense to cast the specified vote in the
    /// given snapshot context (e.g. don't try to add an already authorized signer).
    pub fn is_valid_vote(&self, vote: &Vote) -> bool {
        let signer_is_authorized = self.signers.contains(&vote.address);
        (signer_is_authorized && !vote.authorize) || (!signer_is_authorized && vote.authorize)
    }
}

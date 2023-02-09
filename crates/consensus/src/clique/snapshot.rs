//! Implementation of clique voting snapshots.
//! https://github.com/ethereum/go-ethereum/blob/d0a4989a8def7e6bad182d1513e8d4a093c1672d/consensus/clique/snapshot.go

use super::utils::recover_header_signer;
use bytes::BufMut;
use reth_interfaces::consensus::CliqueError;
use reth_primitives::{
    constants::clique::*, Address, BlockHash, BlockNumber, Bytes, CliqueConfig, SealedHeader, H64,
};
use std::collections::{hash_map::Entry, BTreeSet, HashMap};

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
    /// Create new voting tally.
    pub fn new(authorize: bool) -> Self {
        Self { authorize, votes: 0 }
    }

    /// Increase the vote count by 1.
    pub fn upvote(&mut self) {
        self.votes += 1;
    }

    /// Decrease the vote count by 1.
    pub fn downvote(&mut self) {
        self.votes = self.votes.saturating_sub(1);
    }

    /// Check if the voting tally is empty.
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
    /// Check if the address is an authorized signer.
    pub fn is_authorized_signer(&self, signer: &Address) -> bool {
        self.signers.contains(signer)
    }

    /// Find the recently signed block by the signer.
    pub fn find_recent_signer_block(&self, signer: &Address) -> Option<BlockNumber> {
        self.recents.iter().find_map(|(block, s)| if s == signer { Some(*block) } else { None })
    }

    /// Check if the address is a recent signer.
    pub fn is_recent_signer(&self, signer: &Address) -> bool {
        self.find_recent_signer_block(signer).is_some()
    }

    /// Return a flag if a signer at a given block height is in-turn or not.
    pub fn is_signer_inturn(&self, signer: &Address, block: BlockNumber) -> Option<bool> {
        self.signers
            .iter()
            .position(|s| s == signer)
            .map(|idx| block % self.signers.len() as u64 == idx as u64)
    }

    /// Return the recents cache limit.
    pub fn recents_cache_limit(&self) -> u64 {
        self.signers.len() as u64 / 2 + 1
    }

    /// Return signers as bytes.
    pub fn signers_bytes(&self) -> Bytes {
        let mut bytes = bytes::BytesMut::with_capacity(self.signers.len() * Address::len_bytes());
        for signer in self.signers.iter() {
            bytes.put_slice(signer.as_bytes());
        }
        bytes.freeze().into()
    }

    /// Check whether it makes sense to cast the specified vote in the
    /// given snapshot context (e.g. don't try to add an already authorized signer).
    fn is_valid_vote(&self, vote: &Vote) -> bool {
        let signer_is_authorized = self.is_authorized_signer(&vote.address);
        (signer_is_authorized && !vote.authorize) || (!signer_is_authorized && vote.authorize)
    }

    /// Return true if the vote has passed.
    fn vote_has_passed(&self, tally: &Tally) -> bool {
        let pass_threshold = self.signers.len() / 2;
        tally.votes > pass_threshold as u64
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
        let recents_limit = self.recents_cache_limit();
        if block >= recents_limit {
            let at_block_number = block - recents_limit;
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

// Snapshot voting tests.
// https://github.com/ethereum/go-ethereum/blob/9842301376b4328421e8a6163f12766dfa6641f8/consensus/clique/snapshot_test.go
#[cfg(test)]
mod tests {
    use crate::clique::utils::is_checkpoint_block;

    use super::*;
    use assert_matches::assert_matches;
    use bytes::BytesMut;
    use ethers_core::rand;
    use ethers_signers::{LocalWallet, Signer};
    use itertools::Itertools;
    use reth_primitives::{Header, H256, U256};
    use std::collections::HashSet;

    // Single signer, no votes cast
    #[test]
    fn single_signer_no_votes() {
        SnapshotTest::default()
            .with_signers(vec!["A"])
            .with_votes(vec![TestVote::empty("A")])
            .with_expected_singers(vec!["A"])
            .run()
    }

    // Single signer, voting to add two others (only accept first, second needs 2 votes)
    #[test]
    fn single_signer_two_candidates() {
        SnapshotTest::default()
            .with_signers(vec!["A"])
            .with_votes(vec![
                TestVote::new("A", "B", true),
                TestVote::empty("B"),
                TestVote::new("A", "C", true),
            ])
            .with_expected_singers(vec!["A", "B"])
            .run()
    }

    // Two signers, voting to add three others (only accept first two, third needs 3 votes already)
    #[test]
    fn two_signers_three_candidates() {
        SnapshotTest::default()
            .with_signers(vec!["A", "B"])
            .with_votes(vec![
                TestVote::new("A", "C", true),
                TestVote::new("B", "C", true),
                TestVote::new("A", "D", true),
                TestVote::new("B", "D", true),
                TestVote::empty("C"),
                TestVote::new("A", "E", true),
                TestVote::new("B", "E", true),
            ])
            .with_expected_singers(vec!["A", "B", "C", "D"])
            .run()
    }

    // Single signer, dropping itself
    #[test]
    fn one_signer_one_drop_vote() {
        SnapshotTest::default()
            .with_signers(vec!["A"])
            .with_votes(vec![TestVote::new("A", "A", false)])
            .with_expected_singers(vec![])
            .run()
    }

    // Two signers, actually needing mutual consent to drop either of them (not fulfilled)
    #[test]
    fn two_signers_removing_one_not_fulfilled() {
        SnapshotTest::default()
            .with_signers(vec!["A", "B"])
            .with_votes(vec![TestVote::new("A", "B", false)])
            .with_expected_singers(vec!["A", "B"])
            .run()
    }

    // Two signers, actually needing mutual consent to drop either of them (fulfilled)
    #[test]
    fn two_signers_two_drop_votes() {
        SnapshotTest::default()
            .with_signers(vec!["A", "B"])
            .with_votes(vec![TestVote::new("A", "B", false), TestVote::new("B", "B", false)])
            .with_expected_singers(vec!["A"])
            .run()
    }

    // Three signers, two of them deciding to drop the third
    #[test]
    fn three_signers_two_drop_votes() {
        SnapshotTest::default()
            .with_signers(vec!["A", "B", "C"])
            .with_votes(vec![TestVote::new("A", "C", false), TestVote::new("B", "C", false)])
            .with_expected_singers(vec!["A", "B"])
            .run()
    }

    // Four signers, consensus of two not being enough to drop anyone
    #[test]
    fn four_signers_two_drop_votes() {
        SnapshotTest::default()
            .with_signers(vec!["A", "B", "C", "D"])
            .with_votes(vec![TestVote::new("A", "C", false), TestVote::new("B", "C", false)])
            .with_expected_singers(vec!["A", "B", "C", "D"])
            .run()
    }

    // Four signers, consensus of three already being enough to drop someone
    #[test]
    fn four_signers_three_drop_votes() {
        SnapshotTest::default()
            .with_signers(vec!["A", "B", "C", "D"])
            .with_votes(vec![
                TestVote::new("A", "D", false),
                TestVote::new("B", "D", false),
                TestVote::new("C", "D", false),
            ])
            .with_expected_singers(vec!["A", "B", "C"])
            .run()
    }

    // Authorizations are counted once per signer per target
    #[test]
    fn two_signers_duplicate_vote() {
        SnapshotTest::default()
            .with_signers(vec!["A", "B"])
            .with_votes(vec![
                TestVote::new("A", "C", true),
                TestVote::empty("B"),
                TestVote::new("A", "C", true),
                TestVote::empty("B"),
                TestVote::new("A", "C", true),
            ])
            .with_expected_singers(vec!["A", "B"])
            .run()
    }

    // Authorizing multiple accounts concurrently is permitted
    #[test]
    fn two_signers_concurrent_votes() {
        SnapshotTest::default()
            .with_signers(vec!["A", "B"])
            .with_votes(vec![
                TestVote::new("A", "C", true),
                TestVote::empty("B"),
                TestVote::new("A", "D", true),
                TestVote::empty("B"),
                TestVote::empty("A"),
                TestVote::new("B", "D", true),
                TestVote::empty("A"),
                TestVote::new("B", "C", true),
            ])
            .with_expected_singers(vec!["A", "B", "C", "D"])
            .run()
    }

    // Deauthorizations are counted once per signer per target
    #[test]
    fn two_signers_duplicate_drop_vote() {
        SnapshotTest::default()
            .with_signers(vec!["A", "B"])
            .with_votes(vec![
                TestVote::new("A", "B", false),
                TestVote::empty("B"),
                TestVote::new("A", "B", false),
                TestVote::empty("B"),
                TestVote::new("A", "B", false),
            ])
            .with_expected_singers(vec!["A", "B"])
            .run()
    }

    // Deauthorizing multiple accounts concurrently is permitted
    #[test]
    fn four_signers_concurrent_drop_votes() {
        SnapshotTest::default()
            .with_signers(vec!["A", "B", "C", "D"])
            .with_votes(vec![
                TestVote::new("A", "C", false),
                TestVote::empty("B"),
                TestVote::empty("C"),
                TestVote::new("A", "D", false),
                TestVote::empty("B"),
                TestVote::empty("C"),
                TestVote::empty("A"),
                TestVote::new("B", "D", false),
                TestVote::new("C", "D", false),
                TestVote::empty("A"),
                TestVote::new("B", "C", false),
            ])
            .with_expected_singers(vec!["A", "B"])
            .run()
    }

    // Drop votes from deauthorized signers are discarded immediately
    #[test]
    fn three_signers_drop_vote_discarded() {
        SnapshotTest::default()
            .with_signers(vec!["A", "B", "C"])
            .with_votes(vec![
                TestVote::new("C", "B", false),
                TestVote::new("A", "C", false),
                TestVote::new("B", "C", false),
                TestVote::new("A", "B", false),
            ])
            .with_expected_singers(vec!["A", "B"])
            .run()
    }

    // Votes from deauthorized signers are discarded immediately
    #[test]
    fn three_signers_vote_discarded() {
        SnapshotTest::default()
            .with_signers(vec!["A", "B", "C"])
            .with_votes(vec![
                TestVote::new("C", "D", true),
                TestVote::new("A", "C", false),
                TestVote::new("B", "C", false),
                TestVote::new("A", "D", true),
            ])
            .with_expected_singers(vec!["A", "B"])
            .run()
    }

    // Cascading changes are not allowed, only the account being voted on may change
    #[test]
    fn four_signers_no_cascading_changes() {
        SnapshotTest::default()
            .with_signers(vec!["A", "B", "C", "D"])
            .with_votes(vec![
                TestVote::new("A", "C", false),
                TestVote::empty("B"),
                TestVote::empty("C"),
                TestVote::new("A", "D", false),
                TestVote::new("B", "C", false),
                TestVote::empty("C"),
                TestVote::empty("A"),
                TestVote::new("B", "D", false),
                TestVote::new("C", "D", false),
            ])
            .with_expected_singers(vec!["A", "B", "C"])
            .run()
    }

    // Changes reaching consensus out of bounds (via a deauth) execute on touch
    #[test]
    fn four_signers_cascading_changes_on_touch() {
        SnapshotTest::default()
            .with_signers(vec!["A", "B", "C", "D"])
            .with_votes(vec![
                TestVote::new("A", "C", false),
                TestVote::empty("B"),
                TestVote::empty("C"),
                TestVote::new("A", "D", false),
                TestVote::new("B", "C", false),
                TestVote::empty("C"),
                TestVote::empty("A"),
                TestVote::new("B", "D", false),
                TestVote::new("C", "D", false),
                TestVote::empty("A"),
                TestVote::new("C", "C", true),
            ])
            .with_expected_singers(vec!["A", "B"])
            .run()
    }

    // Changes reaching consensus out of bounds (via a deauth) may go out of consensus on first
    // touch
    #[test]
    fn four_signers_cascading_changes_on_touch_discarded() {
        SnapshotTest::default()
            .with_signers(vec!["A", "B", "C", "D"])
            .with_votes(vec![
                TestVote::new("A", "C", false),
                TestVote::empty("B"),
                TestVote::empty("C"),
                TestVote::new("A", "D", false),
                TestVote::new("B", "C", false),
                TestVote::empty("C"),
                TestVote::empty("A"),
                TestVote::new("B", "D", false),
                TestVote::new("C", "D", false),
                TestVote::empty("A"),
                TestVote::new("B", "C", true),
            ])
            .with_expected_singers(vec!["A", "B", "C"])
            .run()
    }

    // Ensure that pending votes don't survive authorization status changes. This
    // corner case can only appear if a signer is quickly added, removed and then
    // re-added (or the inverse), while one of the original voters dropped. If a
    // past vote is left cached in the system somewhere, this will interfere with
    // the final signer outcome.
    #[test]
    fn five_signers_pending_votes_discarded() {
        SnapshotTest::default()
            .with_signers(vec!["A", "B", "C", "D", "E"])
            .with_votes(vec![
                // Authorize F, 3 votes needed
                TestVote::new("A", "F", true),
                TestVote::new("B", "F", true),
                TestVote::new("C", "F", true),
                // Deauthorize F, 4 votes needed (leave A's previous vote "unchanged")
                TestVote::new("D", "F", false),
                TestVote::new("E", "F", false),
                TestVote::new("B", "F", false),
                TestVote::new("C", "F", false),
                // Almost authorize F, 2/3 votes needed
                TestVote::new("D", "F", true),
                TestVote::new("E", "F", true),
                // Deauthorize A, 3 votes needed
                TestVote::new("B", "A", false),
                TestVote::new("C", "A", false),
                TestVote::new("D", "A", false),
                // Finish authorizing F, 3/3 votes needed
                TestVote::new("B", "F", true),
            ])
            .with_expected_singers(vec!["B", "C", "D", "E", "F"])
            .run()
    }

    // Epoch transitions reset all votes to allow chain checkpointing
    #[test]
    fn two_signers_votes_reset_on_epoch_transition() {
        SnapshotTest::default()
            .with_signers(vec!["A", "B"])
            .with_votes(vec![
                TestVote::new("A", "C", true),
                TestVote::empty("B"),
                TestVote::checkpoint("A", vec!["A", "B"]),
                TestVote::new("B", "C", true),
            ])
            .with_epoch(3)
            .with_expected_singers(vec!["A", "B"])
            .run()
    }

    // An unauthorized signer should not be able to sign blocks
    #[test]
    fn unauthorized_signer_error() {
        SnapshotTest::default()
            .with_signers(vec!["A"])
            .with_votes(vec![TestVote::empty("B")])
            .with_expected_error(|t| CliqueError::UnauthorizedSigner {
                signer: t.get_account_address("B"),
            })
            .run()
    }

    // An authorized signer that signed recently should not be able to sign again
    #[test]
    fn recent_signer_error() {
        SnapshotTest::default()
            .with_signers(vec!["A", "B"])
            .with_votes(vec![TestVote::empty("A"), TestVote::empty("A")])
            .with_expected_error(|t| CliqueError::RecentSigner {
                signer: t.get_account_address("A"),
            })
            .run()
    }

    // Recent signatures should not reset on checkpoint blocks imported in a batch
    #[test]
    fn recent_signers_persist_on_checkpoints() {
        SnapshotTest::default()
            .with_signers(vec!["A", "B", "C"])
            .with_votes(vec![
                TestVote::empty("A"),
                TestVote::empty("B"),
                TestVote::checkpoint("A", vec!["A", "B", "C"]),
                TestVote::empty("A"),
            ])
            .with_expected_error(|t| CliqueError::RecentSigner {
                signer: t.get_account_address("A"),
            })
            .with_epoch(3)
            .run()
    }

    type SignerLabel = String;

    #[derive(Debug, Clone)]
    enum TestVote {
        Empty(
            // Label of the signer who voted.
            SignerLabel,
        ),
        Checkpoint(
            // Label of the signer who voted.
            SignerLabel,
            // The collection of expected signer labels at this block
            Vec<SignerLabel>,
        ),
        Auth(
            // Label of the signer who voted.
            SignerLabel,
            // Label of the signer for whom the vote was cast.
            SignerLabel,
            // Flag indicating whether the vote is authorizing.
            bool,
        ),
    }

    impl TestVote {
        fn new(signer: &str, subject: &str, authorizing: bool) -> Self {
            Self::Auth(signer.to_owned(), subject.to_owned(), authorizing)
        }

        fn empty(signer: &str) -> Self {
            Self::Empty(signer.to_owned())
        }

        fn checkpoint(signer: &str, signers: Vec<&str>) -> Self {
            Self::Checkpoint(signer.to_owned(), signers.into_iter().map(str::to_owned).collect())
        }

        fn signer_label(&self) -> &SignerLabel {
            match self {
                TestVote::Auth(signer, _, _) |
                TestVote::Checkpoint(signer, _) |
                TestVote::Empty(signer) => signer,
            }
        }
    }

    struct SnapshotTest {
        accounts: HashMap<SignerLabel, LocalWallet>,
        config: CliqueConfig,
        signers: HashSet<SignerLabel>,
        votes: Vec<TestVote>,
        expected_result: Result<Vec<SignerLabel>, CliqueError>,
    }

    impl Default for SnapshotTest {
        fn default() -> Self {
            Self {
                accounts: HashMap::default(),
                votes: Vec::default(),
                signers: Default::default(),
                expected_result: Ok(Vec::default()),
                config: CliqueConfig { period: 1, epoch: EPOCH_LENGTH },
            }
        }
    }

    impl SnapshotTest {
        fn with_signers(mut self, signers: Vec<&str>) -> Self {
            for signer in signers {
                self.ensure_account_exists(signer);
                self.signers.insert(signer.to_owned());
            }
            self
        }

        fn with_votes(mut self, votes: Vec<TestVote>) -> Self {
            for vote in votes {
                match &vote {
                    TestVote::Auth(signer, subject, _) => {
                        self.ensure_account_exists(signer);
                        self.ensure_account_exists(subject);
                    }
                    TestVote::Checkpoint(signer, _) | TestVote::Empty(signer) => {
                        self.ensure_account_exists(signer)
                    }
                }
                self.votes.push(vote);
            }
            self
        }

        fn with_epoch(mut self, epoch: u64) -> Self {
            self.config.epoch = epoch;
            self
        }

        fn with_expected_singers(mut self, signers: Vec<&str>) -> Self {
            let mut labels = Vec::with_capacity(signers.len());
            for account in signers {
                self.ensure_account_exists(account);
                labels.push(account.to_owned());
            }
            self.expected_result = Ok(labels);
            self
        }

        fn with_expected_error<F: Fn(&Self) -> CliqueError>(mut self, error_fn: F) -> Self {
            self.expected_result = Err(error_fn(&self));
            self
        }

        // Generate random private key for a label if it doesn't exist yet.
        fn ensure_account_exists(&mut self, label: &str) {
            self.accounts
                .entry(label.to_owned())
                .or_insert(LocalWallet::new(&mut rand::thread_rng()));
        }

        fn get_account(&self, label: &str) -> &LocalWallet {
            self.accounts.get(label).expect("must exist")
        }

        fn get_account_address(&self, label: &str) -> Address {
            Address::from(self.get_account(label).address().as_fixed_bytes())
        }

        fn get_sorted_signers(&self) -> Vec<Address> {
            self.signers.iter().map(|label| self.get_account_address(label)).sorted().collect()
        }

        fn genesis(&self) -> SealedHeader {
            // Construct genesis block
            let mut extra_data = BytesMut::with_capacity(
                EXTRA_VANITY + self.signers.len() * Address::len_bytes() + EXTRA_SEAL,
            );
            extra_data.put_bytes(0, EXTRA_VANITY);
            for signer in self.get_sorted_signers() {
                extra_data.put(signer.as_bytes());
            }
            extra_data.put_bytes(0, EXTRA_SEAL);

            let mut genesis = Header::default();
            genesis.extra_data = extra_data.freeze().into();
            genesis.clique_seal_slow()
        }

        fn construct_clique_header(
            &self,
            number: BlockNumber,
            parent: H256,
            vote: &TestVote,
        ) -> SealedHeader {
            let mut header = Header::default();
            header.mix_hash = rand::random();
            header.number = number;
            header.parent_hash = parent;
            header.difficulty = U256::from(DIFF_INTURN);
            if let TestVote::Auth(_, subject, authorizing) = vote {
                header.beneficiary = self.get_account_address(subject);
                header.nonce =
                    H64::from(if *authorizing { NONCE_AUTH_VOTE } else { NONCE_DROP_VOTE })
                        .to_low_u64_ne();
            }

            // Set extra data for sigining
            header.extra_data = match vote {
                TestVote::Checkpoint(_, expected) => {
                    let mut checkpoint_extra_data = BytesMut::with_capacity(
                        EXTRA_VANITY + EXTRA_SEAL + expected.len() * Address::len_bytes(),
                    );
                    checkpoint_extra_data.put_bytes(0, EXTRA_VANITY);
                    expected.iter().map(|label| self.get_account_address(label)).sorted().for_each(
                        |address| {
                            checkpoint_extra_data.put(address.as_slice());
                        },
                    );
                    checkpoint_extra_data.put_bytes(0, EXTRA_SEAL);
                    checkpoint_extra_data.freeze().into()
                }
                _ => {
                    let shallow_extra_data_len = EXTRA_VANITY + EXTRA_SEAL;
                    let mut shallow_extra_data = BytesMut::with_capacity(shallow_extra_data_len);
                    shallow_extra_data.put_bytes(0, shallow_extra_data_len);
                    shallow_extra_data.freeze().into()
                }
            };

            // Get header hash and sign it
            let hash = header.clique_hash_slow();
            let mut signature = self
                .get_account(&vote.signer_label())
                .sign_hash(ethers_core::types::H256::from_slice(hash.as_bytes()));
            signature.v -= 27; // TODO:

            // Construct and set extra data with extra seal (signature)
            let mut extra_data_with_seal =
                BytesMut::from(&header.extra_data[..header.extra_data.len() - EXTRA_SEAL]);
            extra_data_with_seal.put(&signature.to_vec()[..]);
            header.extra_data = extra_data_with_seal.freeze().into();

            header.seal(hash)
        }

        fn run(self) {
            let genesis = self.genesis();

            // Generate random headers
            let mut headers = Vec::with_capacity(self.votes.len());
            for (idx, vote) in self.votes.iter().enumerate() {
                let header = self.construct_clique_header(
                    idx as u64 + 1,
                    headers.last().map(|h: &SealedHeader| h.hash()).unwrap_or(genesis.hash()),
                    &vote,
                );
                headers.push(header);
            }
            let last_header = headers.pop().expect("not empty");

            let signers =
                self.signers.iter().map(|label| self.get_account_address(label)).collect();
            let mut snapshot = Snapshot {
                config: self.config.clone(),
                hash: genesis.hash(),
                number: 0,
                signers,
                recents: Default::default(),
                tally: Default::default(),
                votes: Default::default(),
            };

            // Start applying snapshots for every header but last
            for header in headers {
                let result = snapshot.apply(&header);
                assert_matches!(result, Ok(()));
                assert_eq!(snapshot.number, header.number);
                assert_eq!(snapshot.hash, header.hash());
                if is_checkpoint_block(&self.config, header.number) {
                    assert_eq!(
                        snapshot.signers_bytes(),
                        Bytes::from(
                            &header.extra_data[EXTRA_VANITY..header.extra_data.len() - EXTRA_SEAL]
                        )
                    );
                }
            }

            // Apply the last header and check against the expected result
            let result = snapshot.apply(&last_header);
            match &self.expected_result {
                Ok(expected_signers) => {
                    assert_eq!(result, Ok(()));
                    assert_eq!(
                        snapshot.signers,
                        expected_signers
                            .iter()
                            .map(|label| self.get_account_address(label))
                            .collect()
                    );
                }
                Err(error) => {
                    assert_eq!(result, Err(error.clone()))
                }
            }
        }
    }
}

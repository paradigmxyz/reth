//! Read and write sets for EVM state.

use derive_more::Deref;
use reth_primitives::{Address, BlockNumber, TransitionId, B256};
use revm::TransitionAccount;
use std::collections::HashSet;

/// The key representing a unique data piece of EVM state.
#[derive(PartialEq, Eq, Hash, Clone, Copy, Debug)]
pub enum RevmKey {
    /// The key representing account and its corresponding information part.
    Account(Address, RevmAccountDataKey),
    /// The key representing a slot.
    Slot(Address, B256),
}

/// The key representing part of account info.
#[derive(PartialEq, Eq, Hash, Clone, Copy, Debug)]
pub enum RevmAccountDataKey {
    /// Account nonce
    Nonce,
    /// Account balance
    Balance,
    /// Account code
    Code,
    /// Account storage. Used for handling the edge case of selfdestruct.
    Storage,
}

/// The access set of revm keys.
#[derive(Deref, Default, Debug)]
pub struct RevmAccessSet(HashSet<RevmKey>);

impl<T> From<T> for RevmAccessSet
where
    T: IntoIterator<Item = RevmKey>,
{
    fn from(value: T) -> Self {
        Self(HashSet::from_iter(value))
    }
}

impl RevmAccessSet {
    /// Record account nonce access.
    pub fn account_nonce(&mut self, address: Address) {
        self.account(address, RevmAccountDataKey::Nonce);
    }

    /// Record account balance access.
    pub fn account_balance(&mut self, address: Address) {
        self.account(address, RevmAccountDataKey::Balance);
    }

    /// Record account code access.
    pub fn account_code(&mut self, address: Address) {
        self.account(address, RevmAccountDataKey::Code);
    }

    /// Record account storage access. Used to denote selfdestruct.
    pub fn account_storage(&mut self, address: Address) {
        self.account(address, RevmAccountDataKey::Storage);
    }

    /// Record account data access.
    pub fn account(&mut self, address: Address, data: RevmAccountDataKey) {
        self.0.insert(RevmKey::Account(address, data));
    }

    /// Record slot access.
    pub fn slot(&mut self, address: Address, slot: B256) {
        self.0.insert(RevmKey::Slot(address, slot));
    }
}

/// The transition read write set.
#[derive(Default, Debug)]
pub struct TransitionRWSet {
    /// The collection of EVM keys read by the transition.
    pub read_set: RevmAccessSet,
    /// The collection of EVM keys written by the transition.
    pub write_set: RevmAccessSet,
    /// Gas used by this transition.
    pub gas_used: u64,
}

impl TransitionRWSet {
    /// Set the read set.
    pub fn with_read_set(mut self, read_set: RevmAccessSet) -> Self {
        self.read_set = read_set;
        self
    }

    /// Set the write set.
    pub fn with_write_set(mut self, write_set: RevmAccessSet) -> Self {
        self.write_set = write_set;
        self
    }

    /// Set the gas used.
    pub fn with_gas_used(mut self, gas_used: u64) -> Self {
        self.gas_used = gas_used;
        self
    }

    /// Record account transition in the write set.
    pub fn record_transition(&mut self, address: Address, transition: &TransitionAccount) {
        // Record account changes.
        let info = transition.info.as_ref();
        let previous_info = transition.previous_info.as_ref();

        if info.map(|info| info.nonce) != previous_info.map(|info| info.nonce) {
            self.write_set.account_nonce(address);
        }

        if info.map(|info| info.balance) != previous_info.map(|info| info.balance) {
            self.write_set.account_balance(address);
        }

        if info.map(|info| info.code_hash) != previous_info.map(|info| info.code_hash) {
            self.write_set.account_code(address);
        }

        // Record storage changes.
        if transition.storage_was_destroyed {
            self.write_set.account_storage(address);
        }

        for (slot, value) in &transition.storage {
            if value.is_changed() {
                self.write_set.slot(address, (*slot).into());
            }
        }
    }

    /// Returns `true` if the read set of the current set depends on the write set of the other.
    pub fn depends_on(&self, other: &Self) -> bool {
        for read_key in self.read_set.iter() {
            // Handle a special case of where the account might have been self destructed.
            if let RevmKey::Slot(address, _) = read_key {
                if other
                    .write_set
                    .contains(&RevmKey::Account(*address, RevmAccountDataKey::Storage))
                {
                    return true
                }
            }

            if other.write_set.contains(read_key) {
                return true
            }
        }

        false
    }

    /// Extend the rw set with contents of another.
    pub fn extend(&mut self, other: Self) {
        self.read_set.0.extend(other.read_set.0.into_iter());
        self.write_set.0.extend(other.write_set.0.into_iter());
    }
}

/// Block read and write set.
#[derive(Default, Debug)]
pub struct BlockRWSet {
    /// Pre block RW set.
    pub pre_block: Option<TransitionRWSet>,
    /// The ordered list of transaction RW sets.
    pub transactions: Vec<TransitionRWSet>,
    /// Post block RW set.
    pub post_block: Option<TransitionRWSet>,
}

impl BlockRWSet {
    /// Create new block rw set from transaction sets.
    pub fn new(transactions: Vec<TransitionRWSet>) -> Self {
        Self { transactions, pre_block: None, post_block: None }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self::new(Vec::with_capacity(capacity))
    }

    pub fn with_pre_block(mut self, pre_block: TransitionRWSet) -> Self {
        self.set_pre_block(Some(pre_block));
        self
    }

    pub fn with_post_block(mut self, post_block: TransitionRWSet) -> Self {
        self.set_post_block(Some(post_block));
        self
    }

    pub fn set_pre_block(&mut self, pre_block: Option<TransitionRWSet>) {
        self.pre_block = pre_block;
    }

    pub fn set_post_block(&mut self, post_block: Option<TransitionRWSet>) {
        self.post_block = post_block;
    }

    pub fn transitions(
        &self,
        block_number: BlockNumber,
    ) -> impl Iterator<Item = (TransitionId, &TransitionRWSet)> {
        self.pre_block
            .as_ref()
            .map(|pre| (TransitionId::pre_block(block_number), pre))
            .into_iter()
            .chain(self.transactions.iter().enumerate().map(move |(idx, rw_set)| {
                (TransitionId::transaction(block_number, idx as u32), rw_set)
            }))
            .chain(
                self.post_block.as_ref().map(|post| (TransitionId::post_block(block_number), post)),
            )
    }

    pub fn into_transitions(
        self,
        block_number: BlockNumber,
    ) -> impl Iterator<Item = (TransitionId, TransitionRWSet)> {
        self.pre_block
            .map(|pre| (TransitionId::pre_block(block_number), pre))
            .into_iter()
            .chain(self.transactions.into_iter().enumerate().map(move |(idx, rw_set)| {
                (TransitionId::transaction(block_number, idx as u32), rw_set)
            }))
            .chain(self.post_block.map(|post| (TransitionId::post_block(block_number), post)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rw_set_dependencies() {
        let account_key = RevmKey::Account(Address::random(), RevmAccountDataKey::Balance);
        let set1 = TransitionRWSet::default().with_read_set(RevmAccessSet::from([account_key]));
        let set2 = TransitionRWSet::default().with_write_set(RevmAccessSet::from([account_key]));
        assert!(set1.depends_on(&set2));
        assert!(!set2.depends_on(&set1));
        assert!(!set2.depends_on(&set2));
        assert!(!set1.depends_on(&set1));

        let address = Address::random();
        let address_storage_key = RevmKey::Account(address, RevmAccountDataKey::Storage);
        let slot_key = RevmKey::Slot(address, B256::random());
        let set1 = TransitionRWSet::default().with_read_set(RevmAccessSet::from([slot_key]));
        let set2 =
            TransitionRWSet::default().with_write_set(RevmAccessSet::from([address_storage_key]));
        assert!(set1.depends_on(&set2));
        assert!(!set2.depends_on(&set1));
        assert!(!set2.depends_on(&set2));
        assert!(!set1.depends_on(&set1));
    }
}

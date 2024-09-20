//! Identifier types for transactions and senders.
use alloy_primitives::Address;
use rustc_hash::FxHashMap;
use std::collections::HashMap;

/// An internal mapping of addresses.
///
/// This assigns a _unique_ [`SenderId`] for a new [`Address`].
/// It has capacity for 2^64 unique addresses.
#[derive(Debug, Default)]
pub struct SenderIdentifiers {
    /// The identifier to use next.
    id: u64,
    /// Assigned [`SenderId`] for an [`Address`].
    address_to_id: HashMap<Address, SenderId>,
    /// Reverse mapping of [`SenderId`] to [`Address`].
    sender_to_address: FxHashMap<SenderId, Address>,
}

impl SenderIdentifiers {
    /// Returns the address for the given identifier.
    #[allow(dead_code)]
    pub fn address(&self, id: &SenderId) -> Option<&Address> {
        self.sender_to_address.get(id)
    }

    /// Returns the [`SenderId`] that belongs to the given address, if it exists
    pub fn sender_id(&self, addr: &Address) -> Option<SenderId> {
        self.address_to_id.get(addr).copied()
    }

    /// Returns the existing [`SenderId`] or assigns a new one if it's missing
    pub fn sender_id_or_create(&mut self, addr: Address) -> SenderId {
        self.sender_id(&addr).unwrap_or_else(|| {
            let id = self.next_id();
            self.address_to_id.insert(addr, id);
            self.sender_to_address.insert(id, addr);
            id
        })
    }

    /// Returns the current identifier and increments the counter.
    fn next_id(&mut self) -> SenderId {
        let id = self.id;
        self.id = self.id.wrapping_add(1);
        id.into()
    }
}

/// A _unique_ identifier for a sender of an address.
///
/// This is the identifier of an internal `address` mapping that is valid in the context of this
/// program.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SenderId(u64);

impl SenderId {
    /// Returns a `Bound` for [`TransactionId`] starting with nonce `0`
    pub const fn start_bound(self) -> std::ops::Bound<TransactionId> {
        std::ops::Bound::Included(TransactionId::new(self, 0))
    }
}

impl From<u64> for SenderId {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

/// A unique identifier of a transaction of a Sender.
///
/// This serves as an identifier for dependencies of a transaction:
/// A transaction with a nonce higher than the current state nonce depends on `tx.nonce - 1`.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct TransactionId {
    /// Sender of this transaction
    pub sender: SenderId,
    /// Nonce of this transaction
    pub nonce: u64,
}

impl TransactionId {
    /// Create a new identifier pair
    pub const fn new(sender: SenderId, nonce: u64) -> Self {
        Self { sender, nonce }
    }

    /// Returns the [`TransactionId`] this transaction depends on.
    ///
    /// This returns `transaction_nonce - 1` if `transaction_nonce` is higher than the
    /// `on_chain_nonce`
    pub fn ancestor(transaction_nonce: u64, on_chain_nonce: u64, sender: SenderId) -> Option<Self> {
        (transaction_nonce > on_chain_nonce)
            .then(|| Self::new(sender, transaction_nonce.saturating_sub(1)))
    }

    /// Returns the [`TransactionId`] that would come before this transaction.
    pub fn unchecked_ancestor(&self) -> Option<Self> {
        (self.nonce != 0).then(|| Self::new(self.sender, self.nonce - 1))
    }

    /// Returns the [`TransactionId`] that directly follows this transaction: `self.nonce + 1`
    pub const fn descendant(&self) -> Self {
        Self::new(self.sender, self.next_nonce())
    }

    /// Returns the nonce that follows immediately after this one.
    #[inline]
    pub const fn next_nonce(&self) -> u64 {
        self.nonce + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn test_transaction_id_new() {
        let sender = SenderId(1);
        let tx_id = TransactionId::new(sender, 5);
        assert_eq!(tx_id.sender, sender);
        assert_eq!(tx_id.nonce, 5);
    }

    #[test]
    fn test_transaction_id_ancestor() {
        let sender = SenderId(1);

        // Special case with nonce 0 and higher on-chain nonce
        let tx_id = TransactionId::ancestor(0, 1, sender);
        assert_eq!(tx_id, None);

        // Special case with nonce 0 and same on-chain nonce
        let tx_id = TransactionId::ancestor(0, 0, sender);
        assert_eq!(tx_id, None);

        // Ancestor is the previous nonce if the transaction nonce is higher than the on-chain nonce
        let tx_id = TransactionId::ancestor(5, 0, sender);
        assert_eq!(tx_id, Some(TransactionId::new(sender, 4)));

        // No ancestor if the transaction nonce is the same as the on-chain nonce
        let tx_id = TransactionId::ancestor(5, 5, sender);
        assert_eq!(tx_id, None);

        // No ancestor if the transaction nonce is lower than the on-chain nonce
        let tx_id = TransactionId::ancestor(5, 15, sender);
        assert_eq!(tx_id, None);
    }

    #[test]
    fn test_transaction_id_unchecked_ancestor() {
        let sender = SenderId(1);

        // Ancestor is the previous nonce if transaction nonce is higher than 0
        let tx_id = TransactionId::new(sender, 5);
        assert_eq!(tx_id.unchecked_ancestor(), Some(TransactionId::new(sender, 4)));

        // No ancestor if transaction nonce is 0
        let tx_id = TransactionId::new(sender, 0);
        assert_eq!(tx_id.unchecked_ancestor(), None);
    }

    #[test]
    fn test_transaction_id_descendant() {
        let sender = SenderId(1);
        let tx_id = TransactionId::new(sender, 5);
        let descendant = tx_id.descendant();
        assert_eq!(descendant, TransactionId::new(sender, 6));
    }

    #[test]
    fn test_transaction_id_next_nonce() {
        let sender = SenderId(1);
        let tx_id = TransactionId::new(sender, 5);
        assert_eq!(tx_id.next_nonce(), 6);
    }

    #[test]
    fn test_transaction_id_ord_eq_sender() {
        let tx1 = TransactionId::new(100u64.into(), 0u64);
        let tx2 = TransactionId::new(100u64.into(), 1u64);
        assert!(tx2 > tx1);
        let set = BTreeSet::from([tx1, tx2]);
        assert_eq!(set.into_iter().collect::<Vec<_>>(), vec![tx1, tx2]);
    }

    #[test]
    fn test_transaction_id_ord() {
        let tx1 = TransactionId::new(99u64.into(), 0u64);
        let tx2 = TransactionId::new(100u64.into(), 1u64);
        assert!(tx2 > tx1);
        let set = BTreeSet::from([tx1, tx2]);
        assert_eq!(set.into_iter().collect::<Vec<_>>(), vec![tx1, tx2]);
    }

    #[test]
    fn test_address_retrieval() {
        let mut identifiers = SenderIdentifiers::default();
        let address = Address::new([1; 20]);
        let id = identifiers.sender_id_or_create(address);
        assert_eq!(identifiers.address(&id), Some(&address));
    }

    #[test]
    fn test_sender_id_retrieval() {
        let mut identifiers = SenderIdentifiers::default();
        let address = Address::new([1; 20]);
        let id = identifiers.sender_id_or_create(address);
        assert_eq!(identifiers.sender_id(&address), Some(id));
    }

    #[test]
    fn test_sender_id_or_create_existing() {
        let mut identifiers = SenderIdentifiers::default();
        let address = Address::new([1; 20]);
        let id1 = identifiers.sender_id_or_create(address);
        let id2 = identifiers.sender_id_or_create(address);
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_sender_id_or_create_new() {
        let mut identifiers = SenderIdentifiers::default();
        let address1 = Address::new([1; 20]);
        let address2 = Address::new([2; 20]);
        let id1 = identifiers.sender_id_or_create(address1);
        let id2 = identifiers.sender_id_or_create(address2);
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_next_id_wrapping() {
        let mut identifiers = SenderIdentifiers { id: u64::MAX, ..Default::default() };

        // The current ID is `u64::MAX`, the next ID should wrap around to 0.
        let id1 = identifiers.next_id();
        assert_eq!(id1, SenderId(u64::MAX));

        // The next ID should now be 0 because of wrapping.
        let id2 = identifiers.next_id();
        assert_eq!(id2, SenderId(0));

        // And then 1, continuing incrementing.
        let id3 = identifiers.next_id();
        assert_eq!(id3, SenderId(1));
    }

    #[test]
    fn test_sender_id_start_bound() {
        let sender = SenderId(1);
        let start_bound = sender.start_bound();
        if let std::ops::Bound::Included(tx_id) = start_bound {
            assert_eq!(tx_id, TransactionId::new(sender, 0));
        } else {
            panic!("Expected included bound");
        }
    }
}

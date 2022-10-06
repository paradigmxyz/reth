use crate::U256;
use fnv::FnvHashMap;
use reth_primitives::Address;
use std::collections::HashMap;

/// An internal mapping of addresses.
///
/// This assigns a _unique_ `SenderId` for a new `Address`.
#[derive(Debug)]
pub struct SenderIdentifiers {
    /// The identifier to use next.
    id: u64,
    /// Assigned `SenderId` for an `Address`.
    address_to_id: HashMap<Address, SenderId>,
    /// Reverse mapping of `SenderId` to `Address`.
    sender_to_address: FnvHashMap<SenderId, Address>,
}

impl SenderIdentifiers {
    /// Returns the address for the given identifier.
    pub fn address(&self, id: &SenderId) -> Option<&Address> {
        self.sender_to_address.get(id)
    }

    /// Returns the `SenderId` that belongs to the given address, if it exists
    pub fn sender_id(&self, addr: &Address) -> Option<SenderId> {
        self.address_to_id.get(addr).copied()
    }

    /// Returns the existing `SendId` or assigns a new one if it's missing
    pub fn sender_id_or_create(&mut self, addr: Address) -> SenderId {
        if let Some(id) = self.sender_id(&addr) {
            return id
        }
        let id = self.next_id();
        self.address_to_id.insert(addr, id);
        self.sender_to_address.insert(id, addr);
        id
    }

    /// Returns a new address
    fn next_id(&mut self) -> SenderId {
        let id = self.id;
        self.id = self.id.wrapping_add(1);
        SenderId(id)
    }
}

/// A _unique_ identifier for a sender of an address.
///
/// This is the identifier of an internal `address` mapping that is valid in the context of this
/// program.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SenderId(u64);

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

// === impl TransactionId ===

impl TransactionId {
    /// Create a new identifier pair
    pub fn new(sender: SenderId, nonce: u64) -> Self {
        Self { sender, nonce }
    }

    /// Returns the id a transactions depends on
    ///
    /// This returns `transaction_nonce - 1` if `transaction_nonce` is higher than the
    /// `on_chain_none`
    pub fn dependency(
        transaction_nonce: u64,
        on_chain_nonce: u64,
        sender: SenderId,
    ) -> Option<TransactionId> {
        if transaction_nonce == on_chain_nonce {
            return None
        }
        let prev_nonce = transaction_nonce.saturating_sub(1);
        if on_chain_nonce <= prev_nonce {
            Some(Self::new(sender, prev_nonce))
        } else {
            None
        }
    }
}

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

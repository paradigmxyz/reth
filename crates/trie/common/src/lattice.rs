//! Commonware LtHash state root helpers.

use alloc::vec::Vec;
use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_primitives::{map::B256Map, B256, U256};
use commonware_codec::{FixedSize, Read, Write};
use commonware_cryptography::lthash::LtHash;
use reth_primitives_traits::Account;

const STORAGE_SLOT_DOMAIN: u8 = 0;
const ACCOUNT_DOMAIN: u8 = 1;

/// Size in bytes of a serialized Commonware LtHash accumulator.
pub const LATTICE_HASH_STATE_BYTES: usize = <LtHash as FixedSize>::SIZE;

/// Serialized Commonware LtHash accumulator state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LatticeHashState([u8; LATTICE_HASH_STATE_BYTES]);

#[cfg(feature = "serde")]
impl serde::Serialize for LatticeHashState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_bytes(self.as_slice())
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for LatticeHashState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bytes = <Vec<u8> as serde::Deserialize<'de>>::deserialize(deserializer)?;
        Self::from_slice(&bytes).map_err(serde::de::Error::custom)
    }
}

impl LatticeHashState {
    /// Creates a lattice hash state from raw bytes.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, &'static str> {
        let state = bytes.try_into().map_err(|_| "invalid lattice hash state length")?;
        Ok(Self(state))
    }

    /// Returns the encoded LtHash bytes.
    pub const fn as_slice(&self) -> &[u8] {
        &self.0
    }

    /// Converts the state into an owned byte vector.
    pub fn into_vec(self) -> Vec<u8> {
        self.0.into()
    }

    /// Reads the Commonware LtHash accumulator represented by this state.
    pub fn to_hash(&self) -> Result<LtHash, &'static str> {
        let mut bytes = self.as_slice();
        LtHash::read_cfg(&mut bytes, &()).map_err(|_| "invalid lattice hash state")
    }

    /// Creates a serialized state from a Commonware LtHash accumulator.
    pub fn from_hash(hash: &LtHash) -> Self {
        let mut bytes = Vec::with_capacity(LATTICE_HASH_STATE_BYTES);
        hash.write(&mut bytes);
        debug_assert_eq!(bytes.len(), LATTICE_HASH_STATE_BYTES);
        Self::from_slice(&bytes).expect("LtHash writes fixed-size state")
    }
}

impl Default for LatticeHashState {
    fn default() -> Self {
        Self::from_hash(&LtHash::default())
    }
}

/// Lattice accumulator state that should be persisted after a block is canonicalized.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LatticeAccumulatorUpdates {
    /// Final global account-state accumulator.
    pub state: LatticeHashState,
    /// Final per-account storage accumulators. `None` removes an empty storage accumulator.
    pub storage: B256Map<Option<LatticeHashState>>,
}

impl LatticeAccumulatorUpdates {
    /// Creates new accumulator updates.
    pub const fn new(state: LatticeHashState, storage: B256Map<Option<LatticeHashState>>) -> Self {
        Self { state, storage }
    }

    /// Returns true if this update only contains the zero global accumulator and no storage
    /// changes.
    pub fn is_empty(&self) -> bool {
        self.state == LatticeHashState::default() && self.storage.is_empty()
    }
}

/// LtHash accumulator for one account's storage slots.
#[derive(Clone, Debug, Default)]
pub struct LatticeStorageRoot {
    hash: LtHash,
    buf: Vec<u8>,
}

impl LatticeStorageRoot {
    /// Creates a storage accumulator from serialized state.
    pub fn from_state(state: &LatticeHashState) -> Result<Self, &'static str> {
        Ok(Self { hash: state.to_hash()?, buf: Vec::new() })
    }

    /// Adds a storage slot to the storage accumulator.
    pub fn add_slot(&mut self, hashed_slot: B256, value: U256) {
        encode_storage_slot(&mut self.buf, hashed_slot, value);
        self.hash.add(&self.buf);
    }

    /// Subtracts a storage slot from the storage accumulator.
    pub fn subtract_slot(&mut self, hashed_slot: B256, value: U256) {
        encode_storage_slot(&mut self.buf, hashed_slot, value);
        self.hash.subtract(&self.buf);
    }

    /// Resets the storage accumulator to the empty state.
    pub const fn reset(&mut self) {
        self.hash.reset();
    }

    /// Returns true if the accumulator is in its zero state.
    pub fn is_zero(&self) -> bool {
        self.hash.is_zero()
    }

    /// Returns the full serialized LtHash state.
    pub fn state(&self) -> LatticeHashState {
        LatticeHashState::from_hash(&self.hash)
    }

    /// Returns the checksum root for the accumulated storage slots.
    pub fn root(&self) -> B256 {
        checksum(&self.hash)
    }
}

/// LtHash accumulator for the global account state.
#[derive(Clone, Debug, Default)]
pub struct LatticeStateRoot {
    hash: LtHash,
    buf: Vec<u8>,
}

impl LatticeStateRoot {
    /// Creates a state accumulator from serialized state.
    pub fn from_state(state: &LatticeHashState) -> Result<Self, &'static str> {
        Ok(Self { hash: state.to_hash()?, buf: Vec::new() })
    }

    /// Adds an account to the global state accumulator.
    pub fn add_account(&mut self, hashed_address: B256, account: Account, storage_root: B256) {
        encode_account(&mut self.buf, hashed_address, account, storage_root);
        self.hash.add(&self.buf);
    }

    /// Subtracts an account from the global state accumulator.
    pub fn subtract_account(&mut self, hashed_address: B256, account: Account, storage_root: B256) {
        encode_account(&mut self.buf, hashed_address, account, storage_root);
        self.hash.subtract(&self.buf);
    }

    /// Returns the full serialized LtHash state.
    pub fn state(&self) -> LatticeHashState {
        LatticeHashState::from_hash(&self.hash)
    }

    /// Returns the checksum root for the accumulated accounts.
    pub fn root(&self) -> B256 {
        checksum(&self.hash)
    }
}

fn encode_storage_slot(buf: &mut Vec<u8>, hashed_slot: B256, value: U256) {
    buf.clear();
    buf.reserve(1 + 32 + 32);
    buf.push(STORAGE_SLOT_DOMAIN);
    buf.extend_from_slice(hashed_slot.as_slice());
    buf.extend_from_slice(&value.to_be_bytes::<32>());
}

fn encode_account(buf: &mut Vec<u8>, hashed_address: B256, account: Account, storage_root: B256) {
    buf.clear();
    buf.reserve(1 + 32 + 8 + 32 + 32 + 32);
    buf.push(ACCOUNT_DOMAIN);
    buf.extend_from_slice(hashed_address.as_slice());
    buf.extend_from_slice(&account.nonce.to_be_bytes());
    buf.extend_from_slice(&account.balance.to_be_bytes::<32>());
    buf.extend_from_slice(account.bytecode_hash.unwrap_or(KECCAK_EMPTY).as_slice());
    buf.extend_from_slice(storage_root.as_slice());
}

fn checksum(hash: &LtHash) -> B256 {
    B256::from(hash.checksum().0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;

    #[test]
    fn lthash_state_roundtrip() {
        let mut root = LatticeStorageRoot::default();
        root.add_slot(B256::with_last_byte(1), U256::from(2));

        let state = root.state();
        let restored = LatticeStorageRoot::from_state(&state).unwrap();

        assert_eq!(state.as_slice().len(), LATTICE_HASH_STATE_BYTES);
        assert_eq!(root.root(), restored.root());
    }

    #[test]
    fn storage_add_subtract_inverse() {
        let slot = B256::with_last_byte(1);
        let value = U256::from(2);

        let mut root = LatticeStorageRoot::default();
        let empty = root.root();

        root.add_slot(slot, value);
        assert_ne!(root.root(), empty);

        root.subtract_slot(slot, value);
        assert_eq!(root.root(), empty);
        assert!(root.is_zero());
    }

    #[test]
    fn account_add_subtract_inverse() {
        let account = Account {
            nonce: 1,
            balance: U256::from(2),
            bytecode_hash: Some(B256::with_last_byte(3)),
        };
        let hashed_address = B256::with_last_byte(4);
        let storage_root = B256::with_last_byte(5);

        let mut root = LatticeStateRoot::default();
        let empty = root.root();

        root.add_account(hashed_address, account, storage_root);
        assert_ne!(root.root(), empty);

        root.subtract_account(hashed_address, account, storage_root);
        assert_eq!(root.root(), empty);
    }
}

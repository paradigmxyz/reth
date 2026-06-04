//! Commonware LtHash state root helpers.

use alloc::vec::Vec;
use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_primitives::{B256, U256};
use commonware_codec::{FixedSize, Read, Write};
use commonware_cryptography::lthash::LtHash;
use reth_primitives_traits::Account;

const ACCOUNT_DOMAIN: u8 = 0;
const STORAGE_SLOT_DOMAIN: u8 = 1;

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
    /// Final flat account and storage accumulator.
    pub state: LatticeHashState,
}

impl LatticeAccumulatorUpdates {
    /// Creates new accumulator updates.
    pub const fn new(state: LatticeHashState) -> Self {
        Self { state }
    }

    /// Returns true if this update only contains the zero accumulator.
    pub fn is_empty(&self) -> bool {
        self.state == LatticeHashState::default()
    }
}

/// Flat LtHash accumulator for account and storage elements.
#[derive(Clone, Debug, Default)]
pub struct LatticeRoot {
    hash: LtHash,
    buf: Vec<u8>,
}

impl LatticeRoot {
    /// Creates an accumulator from serialized state.
    pub fn from_state(state: &LatticeHashState) -> Result<Self, &'static str> {
        Ok(Self { hash: state.to_hash()?, buf: Vec::new() })
    }

    /// Adds an account element to the accumulator.
    pub fn add_account(&mut self, hashed_address: B256, account: Account) {
        encode_account(&mut self.buf, hashed_address, account);
        self.hash.add(&self.buf);
    }

    /// Subtracts an account element from the accumulator.
    pub fn subtract_account(&mut self, hashed_address: B256, account: Account) {
        encode_account(&mut self.buf, hashed_address, account);
        self.hash.subtract(&self.buf);
    }

    /// Adds a storage slot element to the accumulator.
    pub fn add_slot(&mut self, hashed_address: B256, hashed_slot: B256, value: U256) {
        encode_storage_slot(&mut self.buf, hashed_address, hashed_slot, value);
        self.hash.add(&self.buf);
    }

    /// Subtracts a storage slot element from the accumulator.
    pub fn subtract_slot(&mut self, hashed_address: B256, hashed_slot: B256, value: U256) {
        encode_storage_slot(&mut self.buf, hashed_address, hashed_slot, value);
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

fn encode_storage_slot(buf: &mut Vec<u8>, hashed_address: B256, hashed_slot: B256, value: U256) {
    buf.clear();
    buf.reserve(1 + 32 + 32 + 32);
    buf.push(STORAGE_SLOT_DOMAIN);
    buf.extend_from_slice(hashed_address.as_slice());
    buf.extend_from_slice(hashed_slot.as_slice());
    buf.extend_from_slice(&value.to_be_bytes::<32>());
}

fn encode_account(buf: &mut Vec<u8>, hashed_address: B256, account: Account) {
    buf.clear();
    buf.reserve(1 + 32 + 8 + 32 + 32);
    buf.push(ACCOUNT_DOMAIN);
    buf.extend_from_slice(hashed_address.as_slice());
    buf.extend_from_slice(&account.nonce.to_be_bytes());
    buf.extend_from_slice(&account.balance.to_be_bytes::<32>());
    buf.extend_from_slice(account.bytecode_hash.unwrap_or(KECCAK_EMPTY).as_slice());
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
        let mut root = LatticeRoot::default();
        root.add_slot(B256::with_last_byte(1), B256::with_last_byte(2), U256::from(3));

        let state = root.state();
        let restored = LatticeRoot::from_state(&state).unwrap();

        assert_eq!(state.as_slice().len(), LATTICE_HASH_STATE_BYTES);
        assert_eq!(root.root(), restored.root());
    }

    #[test]
    fn storage_add_subtract_inverse() {
        let slot = B256::with_last_byte(1);
        let value = U256::from(2);

        let hashed_address = B256::with_last_byte(7);
        let mut root = LatticeRoot::default();
        let empty = root.root();

        root.add_slot(hashed_address, slot, value);
        assert_ne!(root.root(), empty);

        root.subtract_slot(hashed_address, slot, value);
        assert_eq!(root.root(), empty);
    }

    #[test]
    fn account_add_subtract_inverse() {
        let account = Account {
            nonce: 1,
            balance: U256::from(2),
            bytecode_hash: Some(B256::with_last_byte(3)),
        };
        let hashed_address = B256::with_last_byte(4);
        let mut root = LatticeRoot::default();
        let empty = root.root();

        root.add_account(hashed_address, account);
        assert_ne!(root.root(), empty);

        root.subtract_account(hashed_address, account);
        assert_eq!(root.root(), empty);
    }
}

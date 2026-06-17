//! Lthash state accumulator types.

use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_primitives::{StorageValue, B256};
use reth_primitives_traits::Account;

/// Size of the full TIP-1078 Lthash accumulator.
pub const LTHASH_ACCUMULATOR_LEN: usize = 2048;

const LTHASH_ACCUMULATOR_LANES: usize = LTHASH_ACCUMULATOR_LEN / 2;

/// Size of an encoded TIP-1078 account element.
pub const LTHASH_ACCOUNT_ELEMENT_LEN: usize = 105;
/// Size of an encoded TIP-1078 storage element.
pub const LTHASH_STORAGE_ELEMENT_LEN: usize = 97;

const LTHASH_ACCOUNT_DOMAIN: u8 = 0x00;
const LTHASH_STORAGE_DOMAIN: u8 = 0x01;

/// Full TIP-1078 Lthash accumulator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LthashAccumulator([u16; LTHASH_ACCUMULATOR_LANES]);

impl LthashAccumulator {
    /// Creates a new accumulator from raw bytes.
    pub const fn new(bytes: [u8; LTHASH_ACCUMULATOR_LEN]) -> Self {
        Self::from_bytes(bytes)
    }

    /// Creates an empty accumulator.
    pub const fn zero() -> Self {
        Self([0; LTHASH_ACCUMULATOR_LANES])
    }

    /// Creates an accumulator from raw bytes.
    pub const fn from_bytes(bytes: [u8; LTHASH_ACCUMULATOR_LEN]) -> Self {
        let mut lanes = [0; LTHASH_ACCUMULATOR_LANES];
        let mut i = 0;
        while i < LTHASH_ACCUMULATOR_LANES {
            lanes[i] = u16::from_le_bytes([bytes[i * 2], bytes[i * 2 + 1]]);
            i += 1;
        }
        Self(lanes)
    }

    /// Returns the raw accumulator bytes.
    pub const fn to_bytes(&self) -> [u8; LTHASH_ACCUMULATOR_LEN] {
        let mut bytes = [0; LTHASH_ACCUMULATOR_LEN];
        let mut i = 0;
        while i < LTHASH_ACCUMULATOR_LANES {
            let lane = self.0[i].to_le_bytes();
            bytes[i * 2] = lane[0];
            bytes[i * 2 + 1] = lane[1];
            i += 1;
        }
        bytes
    }

    /// Converts the accumulator into raw bytes.
    pub const fn into_bytes(self) -> [u8; LTHASH_ACCUMULATOR_LEN] {
        self.to_bytes()
    }

    /// Adds data to the accumulator.
    pub fn add(&mut self, data: impl AsRef<[u8]>) {
        let lanes = Self::expand(data.as_ref());
        for (lane, value) in self.0.iter_mut().zip(lanes) {
            *lane = lane.wrapping_add(value);
        }
    }

    /// Subtracts data from the accumulator.
    pub fn subtract(&mut self, data: impl AsRef<[u8]>) {
        let lanes = Self::expand(data.as_ref());
        for (lane, value) in self.0.iter_mut().zip(lanes) {
            *lane = lane.wrapping_sub(value);
        }
    }

    /// Combines another accumulator into this accumulator.
    pub fn combine(&mut self, other: &Self) {
        for (lane, value) in self.0.iter_mut().zip(other.0.iter().copied()) {
            *lane = lane.wrapping_add(value);
        }
    }

    /// Returns the BLAKE3 checksum of the accumulator state.
    pub fn checksum(&self) -> B256 {
        B256::from(*blake3::hash(&self.to_bytes()).as_bytes())
    }

    /// Returns true if the accumulator has zero state.
    pub fn is_zero(&self) -> bool {
        self.0.iter().all(|lane| *lane == 0)
    }

    /// Expands input data to the accumulator lane width using BLAKE3 XOF.
    fn expand(data: &[u8]) -> [u16; LTHASH_ACCUMULATOR_LANES] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(data);

        let mut bytes = [0; LTHASH_ACCUMULATOR_LEN];
        hasher.finalize_xof().fill(&mut bytes);

        Self::from_bytes(bytes).0
    }
}

impl Default for LthashAccumulator {
    fn default() -> Self {
        Self::zero()
    }
}

/// Encodes a live account element for the TIP-1078 accumulator.
///
/// Empty accounts do not contribute an element.
pub fn lthash_account_element(
    hashed_address: B256,
    account: Account,
) -> Option<[u8; LTHASH_ACCOUNT_ELEMENT_LEN]> {
    if account.is_empty() {
        return None
    }

    let mut element = [0u8; LTHASH_ACCOUNT_ELEMENT_LEN];
    element[0] = LTHASH_ACCOUNT_DOMAIN;
    element[1..33].copy_from_slice(hashed_address.as_slice());
    element[33..41].copy_from_slice(&account.nonce.to_be_bytes());
    element[41..73].copy_from_slice(&account.balance.to_be_bytes::<32>());
    element[73..105].copy_from_slice(account.bytecode_hash.unwrap_or(KECCAK_EMPTY).as_slice());
    Some(element)
}

/// Encodes a nonzero storage element for the TIP-1078 accumulator.
///
/// Zero storage values do not contribute an element.
pub fn lthash_storage_element(
    hashed_address: B256,
    hashed_slot: B256,
    value: StorageValue,
) -> Option<[u8; LTHASH_STORAGE_ELEMENT_LEN]> {
    if value.is_zero() {
        return None
    }

    let mut element = [0u8; LTHASH_STORAGE_ELEMENT_LEN];
    element[0] = LTHASH_STORAGE_DOMAIN;
    element[1..33].copy_from_slice(hashed_address.as_slice());
    element[33..65].copy_from_slice(hashed_slot.as_slice());
    element[65..97].copy_from_slice(&value.to_be_bytes::<32>());
    Some(element)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{b256, U256};

    #[test]
    fn roundtrips_raw_bytes() {
        let mut bytes = [0; LTHASH_ACCUMULATOR_LEN];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = i as u8;
        }

        assert_eq!(LthashAccumulator::from_bytes(bytes).to_bytes(), bytes);
    }

    #[test]
    fn add_and_subtract_cancel() {
        let mut accumulator = LthashAccumulator::zero();
        accumulator.add(b"account");
        assert!(!accumulator.is_zero());

        accumulator.subtract(b"account");
        assert!(accumulator.is_zero());
    }

    #[test]
    fn add_is_commutative() {
        let mut left = LthashAccumulator::zero();
        left.add(b"alice");
        left.add(b"bob");

        let mut right = LthashAccumulator::zero();
        right.add(b"bob");
        right.add(b"alice");

        assert_eq!(left, right);
        assert_eq!(left.checksum(), right.checksum());
    }

    #[test]
    fn checksum_changes_with_state() {
        let empty = LthashAccumulator::zero().checksum();

        let mut accumulator = LthashAccumulator::zero();
        accumulator.add(b"alice");

        assert_ne!(accumulator.checksum(), empty);
    }

    #[test]
    fn encodes_account_element() {
        let hashed_address =
            b256!("0x0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20");
        let account = Account {
            nonce: 7,
            balance: U256::from(1_000_000),
            bytecode_hash: Some(B256::repeat_byte(0x42)),
        };

        let element = lthash_account_element(hashed_address, account).unwrap();

        assert_eq!(element.len(), LTHASH_ACCOUNT_ELEMENT_LEN);
        assert_eq!(element[0], LTHASH_ACCOUNT_DOMAIN);
        assert_eq!(&element[1..33], hashed_address.as_slice());
        assert_eq!(&element[33..41], &7u64.to_be_bytes());
        assert_eq!(&element[41..73], &U256::from(1_000_000).to_be_bytes::<32>());
        assert_eq!(&element[73..105], B256::repeat_byte(0x42).as_slice());
    }

    #[test]
    fn empty_account_has_no_element() {
        assert_eq!(lthash_account_element(B256::ZERO, Account::default()), None);
    }

    #[test]
    fn encodes_storage_element() {
        let hashed_address = B256::repeat_byte(0x11);
        let hashed_slot = B256::repeat_byte(0x22);
        let value = U256::from(42);

        let element = lthash_storage_element(hashed_address, hashed_slot, value).unwrap();

        assert_eq!(element.len(), LTHASH_STORAGE_ELEMENT_LEN);
        assert_eq!(element[0], LTHASH_STORAGE_DOMAIN);
        assert_eq!(&element[1..33], hashed_address.as_slice());
        assert_eq!(&element[33..65], hashed_slot.as_slice());
        assert_eq!(&element[65..97], &value.to_be_bytes::<32>());
    }

    #[test]
    fn zero_storage_has_no_element() {
        assert_eq!(lthash_storage_element(B256::ZERO, B256::ZERO, StorageValue::ZERO), None);
    }
}

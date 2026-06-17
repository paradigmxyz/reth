//! Lthash state accumulator types.

use alloy_primitives::B256;

/// Size of the full TIP-1078 Lthash accumulator.
pub const LTHASH_ACCUMULATOR_LEN: usize = 2048;

const LTHASH_ACCUMULATOR_LANES: usize = LTHASH_ACCUMULATOR_LEN / 2;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}

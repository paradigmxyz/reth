//! Filter map parameters and the mapping math they drive.

use alloy_primitives::B256;
use sha2::{Digest, Sha256};

/// The parameters of the filter map structure.
///
/// Only the source fields are stored. Everything Geth caches in `deriveFields` — map height, maps
/// per epoch, values per map, base row length — is recomputed by the `const fn` accessors below, so
/// a `Params` value cannot be observed in a half-derived state.
///
/// The mapping methods assume a sane parameter set, as Geth's do: `log_map_width` strictly greater
/// than `log_values_per_map` and less than 32, and `log_maps_per_epoch` under 32. Both shipped sets
/// satisfy this. Outside that range the shift widths go out of bounds, where Rust panics on debug
/// assertions and Go would silently yield zero — a divergence from the oracle either way, so it is
/// stated here rather than papered over. [`validate`](Self::validate) deliberately does not check
/// it: it mirrors Geth's `sanitize` exactly.
///
/// Fields are deliberately private: the derived values must stay formulas. Hardcoding
/// [`base_row_length`](Self::base_row_length), in particular, is how a parameter set silently
/// acquires rows that hold nothing (see [`RANGE_TEST_PARAMS`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Params {
    /// The number of bits required to represent the map height.
    log_map_height: u32,
    /// The number of bits required to represent the map width.
    log_map_width: u32,
    /// The number of bits required to represent the number of maps per epoch.
    log_maps_per_epoch: u32,
    /// The number of bits required to represent the number of log values per map.
    log_values_per_map: u32,
    /// The ratio of base row length to the average row length.
    base_row_length_ratio: u32,
    /// The logarithmic growth factor (base 2) of the maximum row length per layer: the row length
    /// on layer `x` is `base_row_length << (log_layer_diff * x)`.
    log_layer_diff: u32,
    /// The number of base row entries grouped together as a single database entry.
    base_row_group_size: u32,
}

impl Params {
    /// The number of bits required to represent the map height.
    pub const fn log_map_height(&self) -> u32 {
        self.log_map_height
    }

    /// The number of bits required to represent the map width.
    pub const fn log_map_width(&self) -> u32 {
        self.log_map_width
    }

    /// The number of bits required to represent the number of maps per epoch.
    pub const fn log_maps_per_epoch(&self) -> u32 {
        self.log_maps_per_epoch
    }

    /// The number of bits required to represent the number of log values per map.
    pub const fn log_values_per_map(&self) -> u32 {
        self.log_values_per_map
    }

    /// The ratio of base row length to the average row length.
    pub const fn base_row_length_ratio(&self) -> u32 {
        self.base_row_length_ratio
    }

    /// The logarithmic growth factor (base 2) of the maximum row length per layer.
    pub const fn log_layer_diff(&self) -> u32 {
        self.log_layer_diff
    }

    /// The number of base row entries grouped together as a single database entry.
    pub const fn base_row_group_size(&self) -> u32 {
        self.base_row_group_size
    }

    /// The number of rows in a filter map.
    pub const fn map_height(&self) -> u32 {
        1 << self.log_map_height
    }

    /// The number of columns in a filter map, and therefore the exclusive upper bound of
    /// [`column_index`](Self::column_index).
    pub const fn map_width(&self) -> u32 {
        1 << self.log_map_width
    }

    /// The number of maps in an epoch.
    pub const fn maps_per_epoch(&self) -> u32 {
        1 << self.log_maps_per_epoch
    }

    /// The number of log values marked on each filter map.
    pub const fn values_per_map(&self) -> u64 {
        1 << self.log_values_per_map
    }

    /// The width, in bits, of the hashed low part of a column index.
    ///
    /// A column index packs the value's position within its map into the high bits and this many
    /// hashed bits into the low bits, which is what lets the reverse transform recover a log value
    /// index from a stored column.
    pub const fn hash_bits(&self) -> u32 {
        self.log_map_width - self.log_values_per_map
    }

    /// The maximum number of log values per row on layer 0.
    ///
    /// Computed in `u64` before narrowing, matching Geth's `deriveFields`.
    pub const fn base_row_length(&self) -> u32 {
        (self.values_per_map() * self.base_row_length_ratio as u64 / self.map_height() as u64)
            as u32
    }

    /// Validates the parameter set, mirroring the checks in Geth's `sanitize`.
    ///
    /// Unlike `sanitize` this is pure — there are no derived fields to fill in.
    pub const fn validate(&self) -> Result<(), ParamsError> {
        if !self.log_map_width.is_multiple_of(8) {
            return Err(ParamsError::MapWidthNotMultipleOfEight(self.log_map_width))
        }
        if self.base_row_group_size == 0 || !self.base_row_group_size.is_power_of_two() {
            return Err(ParamsError::BaseRowGroupSizeNotPowerOfTwo(self.base_row_group_size))
        }
        Ok(())
    }

    /// Returns the row index in which the given log value should be marked on the given map and
    /// mapping layer.
    ///
    /// Row assignments are re-shuffled with a different frequency on each mapping layer — see
    /// [`masked_map_index`](Self::masked_map_index). This allows long sections of short rows on
    /// lower layers to be read with a single contiguous cursor walk, while keeping heavy rows on
    /// higher layers away from each other.
    pub fn row_index(&self, map_index: u32, layer_index: u32, log_value: B256) -> u32 {
        let mut hasher = Sha256::new();
        hasher.update(log_value.as_slice());
        hasher.update(self.masked_map_index(map_index, layer_index).to_le_bytes());
        hasher.update(layer_index.to_le_bytes());
        let hash = hasher.finalize();

        let leading = u32::from_le_bytes(hash[..4].try_into().expect("sha256 output is 32 bytes"));
        leading % self.map_height()
    }

    /// Returns the column index at which the log value at the given log value index should be
    /// marked.
    ///
    /// The result packs `log_value_index % values_per_map` into the high bits and a hash-derived
    /// fold into the low [`hash_bits`](Self::hash_bits) bits. The two shifts of the FNV hash are
    /// not interchangeable: the first shifts the full 64-bit hash and then truncates, the second
    /// truncates to 32 bits and then shifts.
    pub fn column_index(&self, log_value_index: u64, log_value: B256) -> u32 {
        let hash_bits = self.hash_bits();

        let mut hasher = Fnv1a64::new();
        hasher.update(&log_value_index.to_le_bytes());
        hasher.update(log_value.as_slice());
        let hash = hasher.finish();

        let low = ((hash >> (64 - hash_bits)) as u32) ^ ((hash as u32) >> (32 - hash_bits));
        (((log_value_index % self.values_per_map()) as u32).wrapping_shl(hash_bits))
            .wrapping_add(low)
    }

    /// Returns the maximum length filter rows are populated up to on the given mapping layer.
    ///
    /// A log value may be marked on a layer only if the row it maps to there has not yet reached
    /// this length; otherwise it spills to the next layer. A row that is full on one layer can
    /// still be extended on a higher one.
    ///
    /// Growth stops once a layer would remap on every map, which for [`DEFAULT_PARAMS`] yields
    /// `[8, 128, 2048, 8192, 8192, ...]`.
    pub const fn max_row_length(&self, layer_index: u32) -> u32 {
        self.base_row_length() << self.layer_shift(layer_index)
    }

    /// Returns the index used for the row mapping calculation on the given layer.
    ///
    /// Layer 0 clears the low `log_maps_per_epoch` bits, so a value keeps one row for a whole
    /// epoch. Each further layer clears fewer bits and therefore remaps more often, until the mask
    /// is all ones and every map remaps.
    pub const fn masked_map_index(&self, map_index: u32, layer_index: u32) -> u32 {
        map_index & (u32::MAX << (self.log_maps_per_epoch - self.layer_shift(layer_index)))
    }

    /// Returns the epoch that the given map index belongs to.
    pub const fn map_epoch(&self, index: u32) -> u32 {
        index >> self.log_maps_per_epoch
    }

    /// Returns the index of the first map in the given epoch.
    pub const fn first_epoch_map(&self, epoch: u32) -> u32 {
        epoch << self.log_maps_per_epoch
    }

    /// Returns the index of the last map in the given epoch.
    pub const fn last_epoch_map(&self, epoch: u32) -> u32 {
        ((epoch + 1) << self.log_maps_per_epoch) - 1
    }

    /// Returns the start index of the base row group containing the given map index.
    pub const fn map_group_index(&self, index: u32) -> u32 {
        index & !(self.base_row_group_size - 1)
    }

    /// Returns the offset of the given map index within its base row group.
    pub const fn map_group_offset(&self, index: u32) -> u32 {
        index & (self.base_row_group_size - 1)
    }

    /// The per-layer shift shared by [`max_row_length`](Self::max_row_length) and
    /// [`masked_map_index`](Self::masked_map_index), clamped so that a layer never remaps more
    /// often than once per map.
    ///
    /// The clamp is what keeps the shift in `masked_map_index` inside `[0, log_maps_per_epoch]`.
    ///
    /// Widened to 64 bits before multiplying, as Geth's `uint` is: a layer index high enough to
    /// overflow 32 bits would otherwise wrap to a small shift and skip the clamp entirely.
    const fn layer_shift(&self, layer_index: u32) -> u32 {
        let shift = layer_index as u64 * self.log_layer_diff as u64;
        if shift > self.log_maps_per_epoch as u64 {
            self.log_maps_per_epoch
        } else {
            shift as u32
        }
    }
}

/// The parameter set used on mainnet.
pub const DEFAULT_PARAMS: Params = Params {
    log_map_height: 16,
    log_map_width: 24,
    log_maps_per_epoch: 10,
    log_values_per_map: 16,
    base_row_group_size: 32,
    base_row_length_ratio: 8,
    log_layer_diff: 4,
};

/// A parameter set that puts one log value per epoch, giving block-exact tail unindexing in tests.
///
/// The ratio of 16 is load-bearing rather than arbitrary: it holds `base_row_length` at
/// `1 * 16 / 16 = 1`. The mainnet ratio of 8 would floor this map's base row length to zero —
/// rows that hold nothing, and therefore a dead index.
pub const RANGE_TEST_PARAMS: Params = Params {
    log_map_height: 4,
    log_map_width: 24,
    log_maps_per_epoch: 0,
    log_values_per_map: 0,
    base_row_group_size: 32,
    base_row_length_ratio: 16,
    log_layer_diff: 4,
};

/// Error returned by [`Params::validate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ParamsError {
    /// The map width is not a whole number of bytes.
    #[error("log_map_width ({0}) must be a multiple of 8")]
    MapWidthNotMultipleOfEight(u32),
    /// The base row group size is not a power of two, so group indices cannot be masked out.
    #[error("base_row_group_size ({0}) must be a power of 2")]
    BaseRowGroupSizeNotPowerOfTwo(u32),
}

/// The 64-bit FNV-1a hash used to derive column indices.
///
/// Implemented inline rather than pulled in as a dependency: it is nine lines, and the column
/// layout depends on this exact construction.
struct Fnv1a64(u64);

impl Fnv1a64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    const fn new() -> Self {
        Self(Self::OFFSET_BASIS)
    }

    fn update(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= byte as u64;
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    const fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{address_value, topic_value};
    use alloy_primitives::{address, b256};

    /// Two log value hashes derived the way the indexer will derive them.
    fn sample_values() -> [B256; 2] {
        [
            address_value(address!("0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef")),
            topic_value(b256!(
                "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"
            )),
        ]
    }

    #[test]
    fn row_index_stays_within_the_map() {
        for params in [DEFAULT_PARAMS, RANGE_TEST_PARAMS] {
            for value in sample_values() {
                for map_index in [0, 1, 5, 1023, 1024, 1025, 4095, u32::MAX] {
                    for layer_index in 0..=6 {
                        assert!(
                            params.row_index(map_index, layer_index, value) < params.map_height(),
                            "row_index({map_index}, {layer_index}) escaped the map"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn column_index_packs_the_position_above_the_hashed_bits() {
        for params in [DEFAULT_PARAMS, RANGE_TEST_PARAMS] {
            for value in sample_values() {
                for log_value_index in [0, 1, 65535, 65536, 65537, 131_072, 1_234_567, u64::MAX] {
                    let column = params.column_index(log_value_index, value);
                    assert!(column < params.map_width(), "column {column} escaped the map");
                    assert_eq!(
                        column >> params.hash_bits(),
                        (log_value_index % params.values_per_map()) as u32,
                        "column_index({log_value_index}) lost its position bits"
                    );
                }
            }
        }
    }

    /// Guards the row length progression against the stale progression published in EIP-7745,
    /// which differs from Geth's and would diverge the row layout from it.
    #[test]
    fn max_row_length_doubles_per_layer_then_clamps() {
        for params in [DEFAULT_PARAMS, RANGE_TEST_PARAMS] {
            // The first layer whose remap frequency has already reached once-per-map; growth stops
            // there because a shorter epoch cannot be subdivided further.
            let clamp_layer = params.log_maps_per_epoch().div_ceil(params.log_layer_diff());

            for layer_index in 0..clamp_layer {
                assert_eq!(
                    params.max_row_length(layer_index),
                    params.base_row_length() << (layer_index * params.log_layer_diff()),
                    "layer {layer_index} grew off the doubling curve"
                );
            }
            let clamped = params.base_row_length() << params.log_maps_per_epoch();
            for layer_index in clamp_layer..clamp_layer + 4 {
                assert_eq!(
                    params.max_row_length(layer_index),
                    clamped,
                    "layer {layer_index} kept growing past the clamp"
                );
            }
        }
    }

    #[test]
    fn base_layer_row_mapping_is_stable_across_an_epoch() {
        let params = DEFAULT_PARAMS;
        let first = params.masked_map_index(params.first_epoch_map(3), 0);

        for map_index in params.first_epoch_map(3)..=params.last_epoch_map(3) {
            assert_eq!(
                params.masked_map_index(map_index, 0),
                first,
                "epoch 3 remapped at {map_index}"
            );
        }
        assert_ne!(
            params.masked_map_index(params.first_epoch_map(4), 0),
            first,
            "the mapping survived an epoch boundary"
        );
    }

    /// The concrete values are pinned by the golden vectors; what they cannot express is that the
    /// two halves must always recombine into the index they came from, for every index.
    #[test]
    fn map_group_helpers_split_the_index_without_losing_it() {
        for params in [DEFAULT_PARAMS, RANGE_TEST_PARAMS] {
            for index in [0, 1, 31, 32, 35, 63, 1000, u32::MAX] {
                assert_eq!(
                    params.map_group_index(index) + params.map_group_offset(index),
                    index,
                    "map index {index} did not survive the split"
                );
                assert!(params.map_group_offset(index) < params.base_row_group_size());
            }
        }
    }

    #[test]
    fn validate_rejects_a_map_width_that_is_not_a_whole_number_of_bytes() {
        let mut params = DEFAULT_PARAMS;
        params.log_map_width = 25;
        assert_eq!(params.validate(), Err(ParamsError::MapWidthNotMultipleOfEight(25)));
    }

    #[test]
    fn validate_rejects_a_base_row_group_size_that_cannot_be_masked() {
        for size in [0, 3, 24] {
            let mut params = DEFAULT_PARAMS;
            params.base_row_group_size = size;
            assert_eq!(params.validate(), Err(ParamsError::BaseRowGroupSizeNotPowerOfTwo(size)));
        }
    }

    #[test]
    fn shipped_parameter_sets_validate() {
        assert_eq!(DEFAULT_PARAMS.validate(), Ok(()));
        assert_eq!(RANGE_TEST_PARAMS.validate(), Ok(()));
    }
}

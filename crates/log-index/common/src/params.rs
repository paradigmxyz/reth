//! Row and column mapping algorithms for `FilterMaps` based on EIP-7745.
//! see: <https://eips.ethereum.org/EIPS/eip-7745>

use crate::MAX_LAYERS;
use alloc::vec::Vec;
use alloy_primitives::B256;
use fnv::FnvHasher;

use core::hash::Hasher;
use sha2::{Digest, Sha256};

/// Default parameters used on mainnet
const DEFAULT_PARAMS: LogIndexParams = LogIndexParams {
    log_map_height: 16,
    log_map_width: 24,
    log_maps_per_epoch: 10,
    log_values_per_map: 16,
    base_row_length_ratio: 8,
    log_layer_diff: 4,
};

/// `LogIndex` parameters based on EIP-7745
#[derive(Debug, Clone, PartialEq, Eq, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct LogIndexParams {
    /// Log of map height (rows per map). Default: 16 (65,536 rows)
    pub log_map_height: u32,
    /// Log of map width (columns per row). Default: 24 (16,777,216 columns)
    pub log_map_width: u32,
    /// Log of maps per epoch. Default: 10 (1,024 maps)
    pub log_maps_per_epoch: u32,
    /// Log of values per map. Default: 16 (65,536 values)
    pub log_values_per_map: u32,
    /// baseRowLength / average row length
    pub base_row_length_ratio: u32,
    /// maxRowLength log2 growth per layer
    pub log_layer_diff: u32,
}

impl Default for LogIndexParams {
    fn default() -> Self {
        DEFAULT_PARAMS
    }
}

impl LogIndexParams {
    #[inline]
    /// Get the number of rows per map
    pub const fn map_height(&self) -> u32 {
        1u32 << self.log_map_height
    }

    #[inline]
    /// Get the number of columns per row
    pub const fn map_width(&self) -> u32 {
        1u32 << self.log_map_width
    }

    #[inline]
    /// Get the number of maps per epoch
    pub const fn maps_per_epoch(&self) -> u32 {
        1u32 << self.log_maps_per_epoch
    }

    #[inline]
    /// Get the number of values per map
    pub const fn values_per_map(&self) -> u64 {
        1u64 << self.log_values_per_map
    }

    /// (`2^log_values_per_map`) - 1
    #[inline]
    pub const fn values_per_map_mask(&self) -> u64 {
        (1u64 << self.log_values_per_map) - 1
    }

    #[inline]
    /// Get the base row length
    pub const fn base_row_length(&self) -> u32 {
        ((self.values_per_map() * self.base_row_length_ratio as u64) / self.map_height() as u64)
            as u32
    }

    #[inline]
    /// Get the epoch of the given map index
    pub const fn map_epoch(&self, index: u32) -> u32 {
        index >> self.log_maps_per_epoch
    }

    /// Get the first map index for the given epoch
    #[inline]
    pub const fn first_epoch_map(&self, epoch: u32) -> u32 {
        epoch << self.log_maps_per_epoch
    }

    /// Get the last map index for the given epoch
    #[inline]
    pub const fn last_epoch_map(&self, epoch: u32) -> u32 {
        ((epoch + 1) << self.log_maps_per_epoch) - 1
    }

    /// Calculate the row index following EIP-7745 epoch-based organization.
    /// This ensures rows with the same `row_index` across an epoch are stored together.
    ///
    /// Layout: (epoch * `map_height` + row) * `maps_per_epoch` + `map_in_epoch`
    pub const fn map_row_index(&self, map_index: u32, row_index: u32) -> u64 {
        let epoch_index = map_index >> self.log_maps_per_epoch;
        let map_sub_index = map_index & ((1 << self.log_maps_per_epoch) - 1);

        // (epoch * map_height + row) * maps_per_epoch + map_in_epoch
        ((((epoch_index as u64) << self.log_map_height) + (row_index as u64)) <<
            self.log_maps_per_epoch) +
            (map_sub_index as u64)
    }

    /// Returns the index used for row mapping calculation on the given layer.
    ///
    /// On layer zero the mapping changes once per epoch, then the frequency of
    /// re-mapping increases with every new layer until it reaches the frequency
    /// where it is different for every mapIndex.
    pub const fn masked_map_index(&self, map_index: u32, layer_index: u32) -> u32 {
        let mut log_layer = (layer_index as u64 * self.log_layer_diff as u64) as u32;
        if log_layer > self.log_maps_per_epoch {
            log_layer = self.log_maps_per_epoch;
        }
        let shift = self.log_maps_per_epoch - log_layer;
        let mask = u32::MAX << shift;
        map_index & mask
    }
    /// Calculate the row index in which the given log value should be marked
    /// on the given map and mapping layer.
    ///
    /// Note that row assignments are re-shuffled with a different frequency on each
    /// mapping layer, allowing efficient disk access and Merkle proofs for long
    /// sections of short rows on lower order layers while avoiding putting too many
    /// heavy rows next to each other on higher order layers.
    pub fn row_index(&self, map_index: u32, layer_index: u32, log_value: &B256) -> u32 {
        let mut hasher = Sha256::new();
        hasher.update(log_value.as_slice());

        let masked = self.masked_map_index(map_index, layer_index);

        // pack as LE(masked) || LE(layer)
        let mut enc = [0u8; 8];
        enc[..4].copy_from_slice(&masked.to_le_bytes());
        enc[4..].copy_from_slice(&layer_index.to_le_bytes());
        hasher.update(enc);

        let digest = hasher.finalize();

        // first 4 bytes, little-endian
        let mut first4 = [0u8; 4];
        first4.copy_from_slice(&digest[..4]);
        let row = u32::from_le_bytes(first4);

        row % self.map_height() // map_height() -> u32 in Geth
    }

    /// columnIndex: FNV-1a( LE(lvIndex) || logValue ), folded to u32
    pub fn column_index(&self, lv_index: u64, log_value: &B256) -> u32 {
        let mut hasher = FnvHasher::default();
        hasher.write(&lv_index.to_le_bytes()); // Geth: LittleEndian
        hasher.write(log_value.as_slice());
        let hash = hasher.finish();

        let hash_bits = self.log_map_width - self.log_values_per_map;
        let pos = (lv_index % self.values_per_map()) as u32;

        let part_hi = (hash >> (64 - hash_bits)) as u32;
        let part_mid = (hash >> (32 - hash_bits)) as u32;
        let collision = part_hi.wrapping_add(part_mid) & ((1u32 << hash_bits) - 1);

        (pos << hash_bits) | collision
    }

    /// Returns the maximum row length for a given layer index.
    #[inline]
    pub const fn max_row_length(&self, layer_index: u32) -> u32 {
        let mut log_layer = (layer_index as u64 * self.log_layer_diff as u64) as u32;
        if log_layer > self.log_maps_per_epoch {
            log_layer = self.log_maps_per_epoch;
        }
        self.base_row_length() << log_layer
    }

    /// Returns the list of log value indices potentially matching the given log
    /// value hash in the range of the filter map the row belongs to.
    ///
    /// Note that the list of indices is always sorted and potential duplicates
    /// are removed. Though the column indices are stored in the same order they
    /// were added and therefore the true matches are automatically reverse
    /// transformed in the right order, false positives can ruin this property.
    /// Since these can only be separated from true matches after the combined
    /// pattern matching of the outputs of individual log value matchers and this
    /// pattern matcher assumes a sorted and duplicate-free list of indices, we
    /// should ensure these properties here.
    pub fn potential_matches(&self, row: Vec<u32>, map_index: u32, log_value: &B256) -> Vec<u64> {
        let mut results = Vec::new();
        let map_first = (map_index as u64) << self.log_values_per_map;

        for col in row {
            // potentialMatch := mapFirst + uint64(row[i]>>(logMapWidth-logValuesPerMap))
            let potential =
                map_first + ((col as u64) >> (self.log_map_width - self.log_values_per_map));
            // row[i] == columnIndex(potentialMatch, logValue)
            if col == self.column_index(potential, log_value) {
                results.push(potential);
            }
        }

        // sort.Sort + in-place dedup
        results.sort_unstable();
        results.dedup();
        results
    }

    /// Extract the row index from a `map_row_index`.
    /// This reverses the `map_row_index` calculation.
    pub const fn extract_row_index(&self, map_row_index: u64, map_index: u32) -> u32 {
        // Reverse of map_row_index calculation:
        // map_row_index = ((epoch * map_height + row) * maps_per_epoch) + map_in_epoch
        let epoch = map_index >> self.log_maps_per_epoch;
        let map_in_epoch = map_index & ((1 << self.log_maps_per_epoch) - 1);

        // Solve for row
        let temp = (map_row_index - map_in_epoch as u64) >> self.log_maps_per_epoch;
        (temp - ((epoch as u64) << self.log_map_height)) as u32
    }

    /// Determine which layer a row belongs to based on its current fill count.
    /// Returns the first layer where the row could fit, or MAX_LAYERS-1 if it doesn't fit anywhere.
    pub fn determine_layer(&self, fill_count: usize) -> usize {
        // Find the first layer where this row could fit
        for layer in 0..MAX_LAYERS {
            if fill_count <= self.max_row_length(layer as u32) as usize {
                return layer as usize;
            }
        }
        // If it doesn't fit in any layer, return the last layer
        (MAX_LAYERS - 1) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{b256, Log};

    #[test]
    fn test_row_index_distribution() {
        let params = LogIndexParams::default();
        let value1 = b256!("0000000000000000000000000000000000000000000000000000000000000001");
        let value2 = b256!("0000000000000000000000000000000000000000000000000000000000000002");

        // Same value should always map to same row for same map/layer
        let row1 = params.row_index(0, 0, &value1);
        let row2 = params.row_index(0, 0, &value1);
        assert_eq!(row1, row2);

        // Different values should (likely) map to different rows
        let row3 = params.row_index(0, 0, &value2);
        // Different values should produce different rows in most cases
        // We can't assert they're always different due to possible hash collisions
        // but we can verify they're valid row indices
        assert!(row3 < params.map_height());

        // Different layers should give different row mappings for the same value
        let row_layer0 = params.row_index(0, 0, &value1);
        let row_layer1 = params.row_index(0, 1, &value1);
        let row_layer2 = params.row_index(0, 2, &value1);

        // All should be valid indices
        assert!(row_layer0 < params.map_height());
        assert!(row_layer1 < params.map_height());
        assert!(row_layer2 < params.map_height());

        // At least one pair should differ (extremely unlikely all three are the same)
        assert!(
            row_layer0 != row_layer1 || row_layer1 != row_layer2 || row_layer0 != row_layer2,
            "All three layers produced the same row index, which is extremely unlikely"
        );
    }

    #[test]
    fn test_column_index() {
        let params = LogIndexParams::default();
        let value = b256!("0000000000000000000000000000000000000000000000000000000000000001");

        // Column index should include position information
        let col1 = params.column_index(0, &value);
        let col2 = params.column_index(params.values_per_map(), &value);

        // Both should be in same position (0) but with different hash bits
        assert_eq!(col1 >> 8, 0);
        assert_eq!(col2 >> 8, 0);

        // Different positions
        let col3 = params.column_index(1, &value);
        assert_eq!(col3 >> 8, 1);
    }

    // #[test]
    //     fn test_potential_matches() {
    //         let params = LogIndexParams::default();
    //         let mut false_positives = 0;

    //         for _ in 0..TEST_PM_COUNT {
    //             let mut rng = rng();
    //             let map_index = rng.random::<u32>();
    //             let lv_start = ((map_index as u64) << params.log_values_per_map) as u32;
    //             let mut row_columns = Vec::new();
    //             let mut lv_indices = Vec::with_capacity(TEST_PM_LEN);
    //             let mut lv_hashes = Vec::with_capacity(TEST_PM_LEN + 1);

    //             // Add TEST_PM_LEN single entries with different log value hashes at different
    // indices             for _ in 0..TEST_PM_LEN {
    //                 let lv_index =
    //                     lv_start + rng.random_range(0..params.values_per_map() as u64) as u32;
    //                 lv_indices.push(lv_index);

    //                 let mut lv_hash_bytes = [0u8; 32];
    //                 rng.fill(&mut lv_hash_bytes);
    //                 let lv_hash = B256::from(lv_hash_bytes);
    //                 lv_hashes.push(lv_hash);

    //                 row_columns.push(params.column_index(lv_index, &lv_hash));
    //             }

    //             let mut common_hash_bytes = [0u8; 32];
    //             rng.fill(&mut common_hash_bytes);
    //             let common_hash = B256::from(common_hash_bytes);
    //             lv_hashes.push(common_hash);

    //             for lv_index in lv_start..lv_start + TEST_PM_LEN {
    //                 row_columns.push(params.column_index(lv_index, &common_hash));
    //             }

    //             // Randomly duplicate some entries
    //             for _ in 0..TEST_PM_LEN {
    //                 let random_entry = row_columns[rng.random_range(0..row_columns.len())];
    //                 row_columns.push(random_entry);
    //             }

    //             // Randomly mix up order of elements
    //             for i in (1..row_columns.len()).rev() {
    //                 let j = rng.random_range(0..i);
    //                 row_columns.swap(i, j);
    //             }

    //             // Split up into a list of rows if longer than allowed
    //             let mut rows = Vec::new();
    //             let mut remaining_columns = Some(row_columns);
    //             let mut layer_index = 0;

    //             while let Some(mut columns) = remaining_columns {
    //                 let max_len = params.max_row_length(layer_index) as usize;
    //                 if columns.len() > max_len {
    //                     let rest = columns.split_off(max_len);
    //                     rows.push(FilterMapRow { columns });
    //                     remaining_columns = Some(rest);
    //                 } else {
    //                     rows.push(FilterMapRow { columns });
    //                     remaining_columns = None;
    //                 }
    //                 layer_index += 1;
    //             }

    //             // Check retrieved matches while also counting false positives
    //             for (i, lv_hash) in lv_hashes.iter().enumerate() {
    //                 let matches = params.potential_matches(&rows, map_index, lv_hash).unwrap();

    //                 if i < TEST_PM_LEN {
    //                     // Check single entry match
    //                     assert!(
    //                         !matches.is_empty(),
    //                         "Invalid length of matches (got {}, expected >=1)",
    //                         matches.len()
    //                     );

    //                     let mut found = false;
    //                     for &lvi in &matches {
    //                         if lvi == lv_indices[i] {
    //                             found = true;
    //                         } else {
    //                             false_positives += 1;
    //                         }
    //                     }

    //                     assert!(
    //                         found,
    //                         "Expected match not found (got {:?}, expected {})",
    //                         matches, lv_indices[i]
    //                     );
    //                 } else {
    //                     // Check "long series" match
    //                     assert!(
    //                         matches.len() >= TEST_PM_LEN,
    //                         "Invalid length of matches (got {}, expected >={})",
    //                         matches.len(),
    //                         TEST_PM_LEN
    //                     );

    //                     // Since results are ordered, first TEST_PM_LEN entries should match
    // exactly                     for (j, _) in matches.iter().enumerate().take(TEST_PM_LEN) {
    //                         assert_eq!(
    //                             matches[j],
    //                             lv_start + j,
    //                             "Incorrect match at index {} (got {}, expected {})",
    //                             j,
    //                             matches[j],
    //                             lv_start + j
    //                         );
    //                     }

    //                     // The rest are false positives
    //                     false_positives += matches.len() - TEST_PM_LEN;
    //                 }
    //             }
    //         }
    //         println!("Total false positives: {false_positives}");
    //     }
}

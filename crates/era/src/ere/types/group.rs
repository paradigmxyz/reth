//! `ere` (era execution) file content group.
//!
//! See also <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/ere.md#specification>

use crate::{
    common::file_ops::{EraFileId, EraFileType},
    e2s::{error::E2sError, types::Entry},
    ere::types::execution::{Accumulator, BlockTuple, MAX_BLOCKS_PER_ERE},
};
use alloy_primitives::BlockNumber;

/// `DynamicBlockIndex` record: ['g', '2']
pub const DYNAMIC_BLOCK_INDEX: [u8; 2] = [0x67, 0x32];

/// Minimum number of index components stored per block (header + body).
pub const MIN_COMPONENTS_PER_BLOCK: u64 = 2;

/// Maximum number of index components stored per block
/// (header + body + receipts + difficulty + proof).
pub const MAX_COMPONENTS_PER_BLOCK: u64 = 5;

/// File content in an `ere` file.
///
/// Format:
/// `CompressedHeader+ | CompressedBody+ | CompressedSlimReceipts* | Proof* | TotalDifficulty* |
/// other-entries* | Accumulator? | DynamicBlockIndex`
///
/// See also <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/ere.md#specification>
#[derive(Debug)]
pub struct EreGroup {
    /// Blocks in this `ere` group
    pub blocks: Vec<BlockTuple>,

    /// Other entries that don't fit into the standard per-block categories
    pub other_entries: Vec<Entry>,

    /// Accumulator over the block header records.
    ///
    /// Optional: it is only present for files that contain pre-merge blocks, since
    /// `total-difficulty` stops advancing after the merge.
    pub accumulator: Option<Accumulator>,

    /// Dynamic block index, required
    pub index: DynamicBlockIndex,
}

impl EreGroup {
    /// Create a new [`EreGroup`]
    pub const fn new(
        blocks: Vec<BlockTuple>,
        accumulator: Option<Accumulator>,
        index: DynamicBlockIndex,
    ) -> Self {
        Self { blocks, accumulator, index, other_entries: Vec::new() }
    }

    /// Add another entry to this group
    pub fn add_entry(&mut self, entry: Entry) {
        self.other_entries.push(entry);
    }
}

/// `ere` block index with a dynamic per-block component count.
///
/// Unlike `era1`'s single-offset-per-block index, an `ere` block can carry a variable number of
/// components, so the index stores `component_count` offsets for every block.
///
/// Format: `starting-number | indexes | indexes | ... | component-count | count`
///
/// where each `indexes` group holds the offsets for one block:
/// `header-index | body-index | receipts-index? | difficulty-index? | proof-index?`
///
/// `component-count` is 2-5 depending on which optional components are present. Offsets are `i64`
/// (they point backward to earlier entries); their little-endian bytes match the spec's `uint64`.
///
/// See also <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/ere.md#specification>
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicBlockIndex {
    /// Starting block number
    starting_number: BlockNumber,

    /// Number of index components per block (2-5)
    component_count: u64,

    /// Flattened, block-major offsets: `[h0, b0, (r0)?, (d0)?, (p0)?, h1, b1, ...]`.
    /// Length is `count * component_count`.
    offsets: Vec<i64>,
}

impl DynamicBlockIndex {
    /// Create a new [`DynamicBlockIndex`].
    ///
    /// `offsets` must be block-major with exactly `component_count` entries per block; the encoded
    /// block count is derived as `offsets.len() / component_count`.
    pub const fn new(
        starting_number: BlockNumber,
        component_count: u64,
        offsets: Vec<i64>,
    ) -> Self {
        Self { starting_number, component_count, offsets }
    }

    /// Get the starting block number
    pub const fn starting_number(&self) -> u64 {
        self.starting_number
    }

    /// Get the number of index components stored per block
    pub const fn component_count(&self) -> u64 {
        self.component_count
    }

    /// Get the number of blocks covered by this index
    pub const fn block_count(&self) -> usize {
        if self.component_count == 0 {
            return 0;
        }
        self.offsets.len() / self.component_count as usize
    }

    /// Get all offsets in block-major order
    pub fn offsets(&self) -> &[i64] {
        &self.offsets
    }

    /// Get the `component_count` offsets for a specific block number.
    ///
    /// Returns a slice ordered as
    /// `[header, body, (receipts)?, (difficulty)?, (proof)?]`, or `None` when the block is outside
    /// the range covered by this index.
    pub fn offsets_for_block(&self, block_number: BlockNumber) -> Option<&[i64]> {
        if block_number < self.starting_number || self.component_count == 0 {
            return None;
        }
        let index = (block_number - self.starting_number) as usize;
        let cc = self.component_count as usize;
        let start = index.checked_mul(cc)?;
        let end = start.checked_add(cc)?;
        self.offsets.get(start..end)
    }

    /// Convert to an [`Entry`] for storage in an e2store file.
    ///
    /// Format: `starting-number | offsets... | component-count | count`
    pub fn to_entry(&self) -> Entry {
        let block_count = self.block_count();
        let mut data = Vec::with_capacity(8 + self.offsets.len() * 8 + 16);

        data.extend_from_slice(&self.starting_number.to_le_bytes());
        data.extend(self.offsets.iter().flat_map(|offset| offset.to_le_bytes()));
        data.extend_from_slice(&self.component_count.to_le_bytes());
        data.extend_from_slice(&(block_count as u64).to_le_bytes());

        Entry::new(DYNAMIC_BLOCK_INDEX, data)
    }

    /// Create from an [`Entry`]
    pub fn from_entry(entry: &Entry) -> Result<Self, E2sError> {
        entry.ensure_type(DYNAMIC_BLOCK_INDEX, "DynamicBlockIndex")?;

        // Need at least: starting-number(8) + component-count(8) + count(8) = 24 bytes
        if entry.data.len() < 24 {
            return Err(E2sError::Ssz(
                "DynamicBlockIndex too short: need at least 24 bytes for starting-number, \
                 component-count and count"
                    .to_string(),
            ));
        }

        let data = &entry.data;
        let len = data.len();

        // Count is the last 8 bytes, component-count the 8 before it.
        let count = u64::from_le_bytes(
            data[len - 8..]
                .try_into()
                .map_err(|_| E2sError::Ssz("Failed to read count bytes".to_string()))?,
        ) as usize;

        let component_count = u64::from_le_bytes(
            data[len - 16..len - 8]
                .try_into()
                .map_err(|_| E2sError::Ssz("Failed to read component-count bytes".to_string()))?,
        );

        if !(MIN_COMPONENTS_PER_BLOCK..=MAX_COMPONENTS_PER_BLOCK).contains(&component_count) {
            return Err(E2sError::Ssz(format!(
                "Invalid component-count for DynamicBlockIndex: expected 2-5, got {component_count}"
            )));
        }

        // Derive the offset count from the actual entry length, not the untrusted `count`, so a
        // crafted `count` can't overflow `* 8` and drive a huge `Vec::with_capacity`.
        let offsets_bytes = len - 24; // len >= 24 checked above
        if !offsets_bytes.is_multiple_of(8) {
            return Err(E2sError::Ssz(
                "DynamicBlockIndex offset section is not 8-byte aligned".to_string(),
            ));
        }
        let total_offsets = offsets_bytes / 8;

        // The declared `count` and `component-count` must match the stored offsets exactly.
        if count.checked_mul(component_count as usize) != Some(total_offsets) {
            return Err(E2sError::Ssz(format!(
                "DynamicBlockIndex length mismatch: count {count} * component-count \
                 {component_count} does not equal the {total_offsets} stored offsets"
            )));
        }

        let starting_number = u64::from_le_bytes(
            data[0..8]
                .try_into()
                .map_err(|_| E2sError::Ssz("Failed to read starting_number bytes".to_string()))?,
        );

        let mut offsets = Vec::with_capacity(total_offsets);
        for chunk in data[8..8 + offsets_bytes].as_chunks::<8>().0 {
            let offset = i64::from_le_bytes(*chunk);
            offsets.push(offset);
        }

        Ok(Self { starting_number, component_count, offsets })
    }
}

/// `ere` file identifier
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EreId {
    /// Network configuration name
    pub network_name: String,

    /// First block number in file
    pub start_block: BlockNumber,

    /// Number of blocks in the file
    pub block_count: u32,

    /// Optional hash identifier for this file.
    /// First 4 bytes of the hash of the last block in the file.
    pub hash: Option<[u8; 4]>,

    /// Whether to include era count in filename.
    /// It is used for custom exports when we don't use the max number of items per file.
    pub include_era_count: bool,

    /// Subset profiles applied to this file.
    ///
    /// Kept sorted and deduplicated by the builders so the filename postfix renders in the
    /// spec-mandated alphabetical order. Empty means the default, fully verifiable profile.
    pub profiles: Vec<EreProfile>,
}

impl EreId {
    /// Create a new [`EreId`]
    pub fn new(
        network_name: impl Into<String>,
        start_block: BlockNumber,
        block_count: u32,
    ) -> Self {
        Self {
            network_name: network_name.into(),
            start_block,
            block_count,
            hash: None,
            include_era_count: false,
            profiles: Vec::new(),
        }
    }

    /// Add a hash identifier to [`EreId`]
    pub const fn with_hash(mut self, hash: [u8; 4]) -> Self {
        self.hash = Some(hash);
        self
    }

    /// Include era count in filename, for custom block-per-file exports
    pub const fn with_era_count(mut self) -> Self {
        self.include_era_count = true;
        self
    }

    /// Add a subset [`EreProfile`] to this file, keeping profiles sorted and deduplicated.
    pub fn with_profile(mut self, profile: EreProfile) -> Self {
        self.profiles.push(profile);
        self.normalize_profiles();
        self
    }

    /// Add several subset profiles to this file, keeping profiles sorted and deduplicated.
    pub fn with_profiles(mut self, profiles: impl IntoIterator<Item = EreProfile>) -> Self {
        self.profiles.extend(profiles);
        self.normalize_profiles();
        self
    }

    /// Sort and deduplicate profiles so the filename postfix is deterministic and alphabetical.
    fn normalize_profiles(&mut self) {
        self.profiles.sort_unstable();
        self.profiles.dedup();
    }
}

impl EraFileId for EreId {
    const FILE_TYPE: EraFileType = EraFileType::Ere;

    const ITEMS_PER_ERA: u64 = MAX_BLOCKS_PER_ERE as u64;

    fn network_name(&self) -> &str {
        &self.network_name
    }

    fn start_number(&self) -> u64 {
        self.start_block
    }

    fn count(&self) -> u32 {
        self.block_count
    }

    fn hash(&self) -> Option<[u8; 4]> {
        self.hash
    }

    fn include_era_count(&self) -> bool {
        self.include_era_count
    }

    /// Render the filename, appending any subset-profile postfixes before the extension.
    ///
    /// Default profile: `<network>-<era-number>-<short-block-hash>.ere`.
    /// With profiles: `<network>-<era-number>-<short-block-hash>-<profile>...-.ere`, in
    /// alphabetical profile order, e.g. `mainnet-00000-4bb7de2e-noproofs-noreceipts.ere`.
    ///
    /// See also <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/ere.md#file-name>
    fn to_file_name(&self) -> String {
        let base = Self::FILE_TYPE.format_filename(
            self.network_name(),
            self.era_number(),
            self.hash(),
            self.include_era_count(),
            self.era_count(),
        );

        if self.profiles.is_empty() {
            return base;
        }

        // Insert the `-<profile>` postfixes before the extension. `profiles` is already sorted
        // and deduplicated, so the order matches the spec's alphabetical requirement.
        let extension = Self::FILE_TYPE.extension();
        let stem = base.strip_suffix(extension).unwrap_or(base.as_str());
        let mut name = String::with_capacity(base.len() + self.profiles.len() * 12);
        name.push_str(stem);
        for profile in &self.profiles {
            name.push('-');
            name.push_str(profile.as_str());
        }
        name.push_str(extension);
        name
    }
}

/// A subset profile for an `ere` file, distinguishing non-default contents from the fully
/// verifiable default profile.
///
/// Variants are ordered so that [`EreId::to_file_name`] renders their postfixes alphabetically,
/// as required by the spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EreProfile {
    /// Omits `Proof` entries (`noproofs`).
    NoProofs,
    /// Omits `CompressedSlimReceipts` entries (`noreceipts`).
    NoReceipts,
}

impl EreProfile {
    /// The lower-case ASCII filename postfix for this profile.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoProofs => "noproofs",
            Self::NoReceipts => "noreceipts",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ere::types::execution::{
        CompressedBody, CompressedHeader, CompressedSlimReceipts, TotalDifficulty,
    };
    use alloy_primitives::{B256, U256};

    /// Build an `ere` block tuple from uncompressed sample bytes; the index/group tests only care
    /// about the tuple's presence, not its decoded contents.
    fn sample_block(data_size: usize) -> BlockTuple {
        BlockTuple::new(
            CompressedHeader::new(vec![0xAA; data_size]),
            CompressedBody::new(vec![0xBB; data_size * 2]),
        )
        .with_receipts(CompressedSlimReceipts::new(vec![0xCC; data_size]))
        .with_total_difficulty(TotalDifficulty::new(U256::from(data_size)))
    }

    #[test]
    fn test_dynamic_block_index_roundtrip() {
        let starting_number = 1000;
        let component_count = 4;
        // 2 blocks, 4 components each = 8 offsets
        let offsets = vec![100, 200, 300, 400, 500, 600, 700, 800];

        let block_index = DynamicBlockIndex::new(starting_number, component_count, offsets.clone());

        let entry = block_index.to_entry();
        assert_eq!(entry.entry_type, DYNAMIC_BLOCK_INDEX);

        let recovered = DynamicBlockIndex::from_entry(&entry).unwrap();
        assert_eq!(recovered, block_index);
        assert_eq!(recovered.starting_number(), starting_number);
        assert_eq!(recovered.component_count(), component_count);
        assert_eq!(recovered.offsets(), offsets);
        assert_eq!(recovered.block_count(), 2);
    }

    #[test]
    fn test_dynamic_block_index_negative_offsets_roundtrip() {
        // Offsets point backward from the index to earlier entries, so they are negative in real
        // files. Cover the full component-count range (2 and 5) to exercise the `i64` LE encoding.
        for (component_count, offsets) in [
            (2u64, vec![-2048, -1024, -512, -256]),
            (5u64, vec![-50, -40, -30, -20, -10, -9, -8, -7, -6, -5]),
        ] {
            let index = DynamicBlockIndex::new(1000, component_count, offsets);
            let recovered = DynamicBlockIndex::from_entry(&index.to_entry()).unwrap();
            assert_eq!(recovered, index);
        }
    }

    #[test]
    fn test_dynamic_block_index_offset_lookup() {
        let starting_number = 1000;
        let component_count = 3;
        // 3 blocks, 3 components each = 9 offsets
        let offsets = vec![10, 20, 30, 40, 50, 60, 70, 80, 90];

        let block_index = DynamicBlockIndex::new(starting_number, component_count, offsets);

        // Block 1000: [10, 20, 30]
        assert_eq!(block_index.offsets_for_block(1000), Some(&[10, 20, 30][..]));

        // Block 1002: [70, 80, 90]
        assert_eq!(block_index.offsets_for_block(1002), Some(&[70, 80, 90][..]));

        // Out of range below and above
        assert_eq!(block_index.offsets_for_block(999), None);
        assert_eq!(block_index.offsets_for_block(1003), None);
    }

    #[test]
    fn test_dynamic_block_index_rejects_bad_component_count() {
        // component-count must be in 2..=5; forge an entry with component-count = 1.
        let mut data = Vec::new();
        data.extend_from_slice(&1000u64.to_le_bytes()); // starting-number
        data.extend_from_slice(&42i64.to_le_bytes()); // single offset
        data.extend_from_slice(&1u64.to_le_bytes()); // component-count = 1 (invalid)
        data.extend_from_slice(&1u64.to_le_bytes()); // count
        let entry = Entry::new(DYNAMIC_BLOCK_INDEX, data);

        assert!(DynamicBlockIndex::from_entry(&entry).is_err());
    }

    #[test]
    fn test_dynamic_block_index_rejects_overflowing_count() {
        // 24-byte entry declaring a count whose offset length overflows usize; must be rejected,
        // not allocated.
        let mut data = Vec::new();
        data.extend_from_slice(&1000u64.to_le_bytes()); // starting-number
        data.extend_from_slice(&2u64.to_le_bytes()); // component-count = 2
        data.extend_from_slice(&(1u64 << 60).to_le_bytes()); // count = 2^60
        let entry = Entry::new(DYNAMIC_BLOCK_INDEX, data);

        assert!(DynamicBlockIndex::from_entry(&entry).is_err());
    }

    #[test]
    fn test_dynamic_block_index_rejects_wrong_length() {
        // Encode a valid index, then drop a trailing byte so the declared count no longer matches.
        let block_index = DynamicBlockIndex::new(1000, 2, vec![100, 200, 300, 400]);
        let mut entry = block_index.to_entry();
        entry.data.pop();

        assert!(DynamicBlockIndex::from_entry(&entry).is_err());
    }

    #[test]
    fn test_dynamic_block_index_rejects_wrong_type() {
        let entry = Entry::new([0x66, 0x32], vec![0u8; 24]);
        assert!(DynamicBlockIndex::from_entry(&entry).is_err());
    }

    #[test]
    fn test_ere_group_basic_construction() {
        let blocks = vec![sample_block(10), sample_block(15), sample_block(20)];

        let accumulator = Accumulator::new(B256::from([0xDD; 32]));
        let block_index = DynamicBlockIndex::new(1000, 2, vec![100, 200, 300, 400, 500, 600]);

        let group = EreGroup::new(blocks, Some(accumulator.clone()), block_index);

        assert_eq!(group.blocks.len(), 3);
        assert_eq!(group.other_entries.len(), 0);
        assert_eq!(group.accumulator.unwrap().root, accumulator.root);
        assert_eq!(group.index.starting_number(), 1000);
        assert_eq!(group.index.offsets(), vec![100, 200, 300, 400, 500, 600]);
    }

    #[test]
    fn test_ere_group_without_accumulator() {
        // Post-merge files carry no accumulator.
        let blocks = vec![sample_block(10)];
        let block_index = DynamicBlockIndex::new(1000, 2, vec![100, 200]);

        let group = EreGroup::new(blocks, None, block_index);

        assert!(group.accumulator.is_none());
        assert_eq!(group.blocks.len(), 1);
    }

    #[test]
    fn test_ere_group_add_entries() {
        let blocks = vec![sample_block(10)];
        let accumulator = Accumulator::new(B256::from([0xDD; 32]));
        let block_index = DynamicBlockIndex::new(1000, 2, vec![100, 200]);

        let mut group = EreGroup::new(blocks, Some(accumulator), block_index);
        assert_eq!(group.other_entries.len(), 0);

        group.add_entry(Entry::new([0x01, 0x01], vec![1, 2, 3, 4]));
        group.add_entry(Entry::new([0x02, 0x02], vec![5, 6, 7, 8]));

        assert_eq!(group.other_entries.len(), 2);
        assert_eq!(group.other_entries[0].entry_type, [0x01, 0x01]);
        assert_eq!(group.other_entries[1].data, vec![5, 6, 7, 8]);
    }

    #[test]
    fn test_ere_group_with_mismatched_index() {
        // The group is a plain container; it does not validate that block count matches the index.
        let blocks = vec![sample_block(10), sample_block(15)];
        let index = DynamicBlockIndex::new(2000, 2, vec![100, 200, 300, 400, 500, 600]); // 3 blocks
        let group = EreGroup::new(blocks, None, index);
        assert_eq!(group.blocks.len(), 2);
        assert_eq!(group.index.starting_number(), 2000);
    }

    #[test_case::test_case(
        EreId::new("mainnet", 0, 8192).with_hash([0x5e, 0xc1, 0xff, 0xb8]),
        "mainnet-00000-5ec1ffb8.ere";
        "Mainnet era 0"
    )]
    #[test_case::test_case(
        EreId::new("mainnet", 8192, 8192).with_hash([0x5e, 0xcb, 0x9b, 0xf9]),
        "mainnet-00001-5ecb9bf9.ere";
        "Mainnet era 1"
    )]
    #[test_case::test_case(
        EreId::new("sepolia", 0, 8192).with_hash([0x90, 0x91, 0x84, 0x72]),
        "sepolia-00000-90918472.ere";
        "Sepolia era 0"
    )]
    #[test_case::test_case(
        EreId::new("mainnet", 1000, 100),
        "mainnet-00000-00000000.ere";
        "ID without hash"
    )]
    fn test_ere_id_file_naming(id: EreId, expected_file_name: &str) {
        assert_eq!(id.to_file_name(), expected_file_name);
    }

    // File naming with era-count, for custom exports
    #[test_case::test_case(
        EreId::new("mainnet", 0, 8192).with_hash([0x5e, 0xc1, 0xff, 0xb8]).with_era_count(),
        "mainnet-00000-00001-5ec1ffb8.ere";
        "Mainnet era 0 with count"
    )]
    #[test_case::test_case(
        EreId::new("mainnet", 8000, 500).with_hash([0xab, 0xcd, 0xef, 0x12]).with_era_count(),
        "mainnet-00000-00002-abcdef12.ere";
        "Spanning two eras with count"
    )]
    fn test_ere_id_file_naming_with_era_count(id: EreId, expected_file_name: &str) {
        assert_eq!(id.to_file_name(), expected_file_name);
    }

    // File naming with subset-profile postfixes, in alphabetical order.
    #[test_case::test_case(
        EreId::new("mainnet", 0, 8192).with_hash([0x4b, 0xb7, 0xde, 0x2e]),
        "mainnet-00000-4bb7de2e.ere";
        "Default profile, no postfix"
    )]
    #[test_case::test_case(
        EreId::new("mainnet", 0, 8192).with_hash([0x4b, 0xb7, 0xde, 0x2e]).with_profile(EreProfile::NoProofs),
        "mainnet-00000-4bb7de2e-noproofs.ere";
        "noproofs profile"
    )]
    #[test_case::test_case(
        EreId::new("mainnet", 0, 8192).with_hash([0x4b, 0xb7, 0xde, 0x2e]).with_profile(EreProfile::NoReceipts),
        "mainnet-00000-4bb7de2e-noreceipts.ere";
        "noreceipts profile"
    )]
    #[test_case::test_case(
        EreId::new("mainnet", 0, 8192).with_hash([0x4b, 0xb7, 0xde, 0x2e]).with_profiles([EreProfile::NoProofs, EreProfile::NoReceipts]),
        "mainnet-00000-4bb7de2e-noproofs-noreceipts.ere";
        "Combined profiles"
    )]
    #[test_case::test_case(
        // Insertion order reversed and duplicated; output must still be alphabetical and deduped.
        EreId::new("mainnet", 0, 8192).with_hash([0x4b, 0xb7, 0xde, 0x2e]).with_profile(EreProfile::NoReceipts).with_profile(EreProfile::NoProofs).with_profile(EreProfile::NoReceipts),
        "mainnet-00000-4bb7de2e-noproofs-noreceipts.ere";
        "Profiles normalized to alphabetical order"
    )]
    fn test_ere_id_file_naming_with_profiles(id: EreId, expected_file_name: &str) {
        assert_eq!(id.to_file_name(), expected_file_name);
    }
}

//! Hot account tracking for smart trie preservation.
//!
//! This module provides data structures for tracking frequently-accessed ("hot") accounts
//! across blocks. Hot accounts are preserved in the sparse trie between blocks to avoid
//! redundant proof fetching.
//!
//! # Architecture
//!
//! The tracking uses a tiered approach:
//! - **Tier A (Always)**: System contracts and major defi contracts (static, known)
//! - **Tier B (Likely)**: Builder addresses, fee recipients (semi-static)
//! - **Tier C (Dynamic)**: Accounts detected as hot via rotating bloom filters
//!
//! # Example
//!
//! ```ignore
//! let mut tracker = TieredHotAccounts::for_mainnet();
//!
//! // During block execution, record touched accounts
//! tracker.record(hashed_address, /* has_code */ true);
//!
//! // At end of block, rotate filters
//! tracker.end_block();
//!
//! // Check if account should be preserved
//! if tracker.should_preserve(&hashed_address) {
//!     // Keep in trie
//! }
//! ```

use alloy_primitives::{map::HashSet, B256};
use core::hash::{Hash, Hasher};
use rustc_hash::FxHasher;
use std::collections::VecDeque;

/// Number of hash functions to use in bloom filter.
/// Using 3 hash functions is optimal for ~1% false positive rate.
const BLOOM_HASH_COUNT: usize = 3;

/// Default bloom filter size in bits (8KB = 65536 bits).
/// At 1% FPR with 3 hash functions, this supports ~6000 elements.
const DEFAULT_BLOOM_BITS: usize = 65536;

/// Default number of blocks to track in rotating filter.
const DEFAULT_HISTORY_BLOCKS: usize = 32;

/// Default threshold: account is "hot" if seen in this many recent blocks.
const DEFAULT_HOT_THRESHOLD: usize = 8;

/// Maximum number of recent fee recipients to track (prevents unbounded growth).
const MAX_RECENT_FEE_RECIPIENTS: usize = 128;

/// A simple bloom filter backed by a bit vector.
///
/// Uses multiple hash functions derived from the input to set/check bits.
/// False positives are possible, false negatives are not.
#[derive(Debug, Clone)]
pub struct BloomFilter {
    /// Bit storage as u64 words.
    bits: Vec<u64>,
    /// Number of bits (may not be exactly `bits.len() * 64`).
    num_bits: usize,
    /// Number of hash functions.
    hash_count: usize,
}

impl Default for BloomFilter {
    fn default() -> Self {
        Self::new(DEFAULT_BLOOM_BITS)
    }
}

impl BloomFilter {
    /// Creates a new bloom filter with the specified number of bits.
    pub fn new(num_bits: usize) -> Self {
        let num_words = num_bits.div_ceil(64);
        Self { bits: vec![0u64; num_words], num_bits, hash_count: BLOOM_HASH_COUNT }
    }

    /// Creates a bloom filter sized for an expected number of elements at ~1% FPR.
    pub fn with_capacity(expected_elements: usize) -> Self {
        // Optimal bits = -n * ln(p) / (ln(2)^2) where p = 0.01
        // Simplified: bits ≈ n * 10 for 1% FPR
        let num_bits = (expected_elements * 10).max(1024).next_power_of_two();
        Self::new(num_bits)
    }

    /// Inserts an element into the bloom filter.
    pub fn insert(&mut self, value: &B256) {
        for i in 0..self.hash_count {
            let idx = self.hash_index(value, i);
            let word_idx = idx / 64;
            let bit_idx = idx % 64;
            self.bits[word_idx] |= 1u64 << bit_idx;
        }
    }

    /// Checks if an element is possibly in the filter.
    ///
    /// Returns `true` if the element might be present (could be false positive).
    /// Returns `false` if the element is definitely not present.
    pub fn contains(&self, value: &B256) -> bool {
        (0..self.hash_count).all(|i| {
            let idx = self.hash_index(value, i);
            let word_idx = idx / 64;
            let bit_idx = idx % 64;
            (self.bits[word_idx] & (1u64 << bit_idx)) != 0
        })
    }

    /// Clears all bits in the filter.
    pub fn clear(&mut self) {
        self.bits.fill(0);
    }

    /// Returns the number of bits set in the filter.
    pub fn count_ones(&self) -> usize {
        self.bits.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// Returns the total number of bits in the filter.
    pub const fn len(&self) -> usize {
        self.num_bits
    }

    /// Returns true if no bits are set.
    pub fn is_empty(&self) -> bool {
        self.bits.iter().all(|&w| w == 0)
    }

    /// Computes the bit index for a value using hash function `i`.
    #[inline]
    fn hash_index(&self, value: &B256, i: usize) -> usize {
        // Use different portions of the B256 as hash seeds
        // B256 is 32 bytes, we can extract multiple u64s
        let bytes = value.as_slice();

        // Combine bytes with index to create different hash functions
        let mut hasher = FxHasher::default();
        bytes.hash(&mut hasher);
        (i as u64).hash(&mut hasher);
        let hash = hasher.finish();

        (hash as usize) % self.num_bits
    }
}

/// A rotating bloom filter that tracks elements across multiple time periods (blocks).
///
/// Maintains a ring buffer of bloom filters, one per block. Elements are considered
/// "hot" if they appear in at least `hot_threshold` of the recent blocks.
#[derive(Debug)]
pub struct RotatingBloomFilter {
    /// Current block's filter (being filled).
    current: BloomFilter,
    /// Historical filters (most recent first).
    history: VecDeque<BloomFilter>,
    /// Maximum number of historical blocks to track.
    history_size: usize,
    /// Minimum appearances in recent blocks to be considered hot.
    hot_threshold: usize,
    /// Expected elements per block (for sizing new filters).
    expected_elements: usize,
}

impl Default for RotatingBloomFilter {
    fn default() -> Self {
        Self::new(DEFAULT_HISTORY_BLOCKS, DEFAULT_HOT_THRESHOLD, 1000)
    }
}

impl RotatingBloomFilter {
    /// Creates a new rotating bloom filter.
    ///
    /// # Arguments
    /// * `history_size` - Number of historical blocks to track
    /// * `hot_threshold` - Minimum appearances to be considered hot
    /// * `expected_elements` - Expected elements per block (for filter sizing)
    pub fn new(history_size: usize, hot_threshold: usize, expected_elements: usize) -> Self {
        Self {
            current: BloomFilter::with_capacity(expected_elements),
            history: VecDeque::with_capacity(history_size),
            history_size,
            hot_threshold,
            expected_elements,
        }
    }

    /// Records an element as seen in the current block.
    pub fn insert(&mut self, value: B256) {
        self.current.insert(&value);
    }

    /// Rotates the filter at the end of a block.
    ///
    /// The current filter becomes history, and a new empty filter is created.
    pub fn end_block(&mut self) {
        // Determine size for next filter based on current usage
        let next_expected = self.current.count_ones().max(self.expected_elements);

        // Move current to history
        let prev = std::mem::replace(&mut self.current, BloomFilter::with_capacity(next_expected));
        self.history.push_front(prev);

        // Trim old history
        while self.history.len() > self.history_size {
            self.history.pop_back();
        }
    }

    /// Checks if an element is "hot" (seen frequently in recent blocks).
    pub fn is_hot(&self, value: &B256) -> bool {
        let count = self.history.iter().filter(|f| f.contains(value)).count();
        count >= self.hot_threshold
    }

    /// Checks if an element was seen in the current block.
    pub fn seen_this_block(&self, value: &B256) -> bool {
        self.current.contains(value)
    }

    /// Returns the number of historical blocks being tracked.
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// Handles a reorg by invalidating recent history.
    ///
    /// Removes the most recent `depth` blocks from history.
    pub fn handle_reorg(&mut self, depth: usize) {
        for _ in 0..depth.min(self.history.len()) {
            self.history.pop_front();
        }
        self.current.clear();
    }

    /// Clears all state.
    pub fn clear(&mut self) {
        self.current.clear();
        self.history.clear();
    }

    /// Returns approximate memory usage in bytes.
    pub fn memory_usage(&self) -> usize {
        let current_bytes = self.current.len() / 8;
        let history_bytes: usize = self.history.iter().map(|f| f.len() / 8).sum();
        current_bytes + history_bytes
    }
}

/// Hotness level for an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Hotness {
    /// Cold: not tracked as hot.
    Cold = 0,
    /// Dynamic: detected as hot via bloom filter.
    Dynamic = 1,
    /// Likely: known builder/fee recipient.
    Likely = 2,
    /// Always: system contract or major defi.
    Always = 3,
}

/// A storage slot key combining account and slot hashes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StorageSlotKey {
    /// Hashed account address.
    pub account: B256,
    /// Hashed storage slot.
    pub slot: B256,
}

impl StorageSlotKey {
    /// Creates a new storage slot key.
    pub const fn new(account: B256, slot: B256) -> Self {
        Self { account, slot }
    }

    /// Combines account and slot into a single B256 for bloom filter insertion.
    fn combined(&self) -> B256 {
        let mut combined = [0u8; 32];
        for i in 0..16 {
            combined[i] = self.account.0[i] ^ self.slot.0[i];
            combined[i + 16] = self.account.0[i + 16] ^ self.slot.0[i + 16];
        }
        B256::from(combined)
    }
}

/// Tracks hot storage slots for smart preservation.
///
/// Maintains:
/// - Predictable slots for system contracts (computed per block)
/// - Dynamic tracking of frequently-accessed slots via bloom filter
#[derive(Debug)]
pub struct HotStorageSlots {
    /// Bloom filter for dynamically-detected hot slots.
    dynamic_slots: RotatingBloomFilter,
}

impl Default for HotStorageSlots {
    fn default() -> Self {
        Self::new()
    }
}

impl HotStorageSlots {
    /// Creates a new hot storage slots tracker.
    pub fn new() -> Self {
        Self { dynamic_slots: RotatingBloomFilter::new(32, 4, 1000) }
    }

    /// Records a storage slot access.
    pub fn record(&mut self, account: B256, slot: B256) {
        let key = StorageSlotKey::new(account, slot);
        self.dynamic_slots.insert(key.combined());
    }

    /// Checks if a storage slot is hot.
    pub fn is_hot(&self, account: &B256, slot: &B256) -> bool {
        let key = StorageSlotKey::new(*account, *slot);
        self.dynamic_slots.is_hot(&key.combined())
    }

    /// Rotates filters at end of block.
    pub fn end_block(&mut self) {
        self.dynamic_slots.end_block();
    }

    /// Handles a reorg by invalidating recent history.
    pub fn handle_reorg(&mut self, depth: usize) {
        self.dynamic_slots.handle_reorg(depth);
    }

    /// Clears all state.
    pub fn clear(&mut self) {
        self.dynamic_slots.clear();
    }

    /// Returns predictable storage slots for system contracts at a given block.
    ///
    /// These slots are deterministically computed based on block number/timestamp.
    pub fn predictable_system_slots(block_number: u64, timestamp: u64) -> Vec<StorageSlotKey> {
        use alloy_primitives::keccak256;

        // EIP-4788 Beacon Roots contract
        let beacon_roots_addr =
            keccak256(alloy_primitives::address!("000F3df6D732807Ef1319fB7B8bB8522d0Beac02"));
        // Ring buffer slots: timestamp % 8191 (timestamp slot) and timestamp % 8191 + 8191 (root
        // slot)
        let beacon_ts_slot = B256::from(alloy_primitives::U256::from(timestamp % 8191));
        let beacon_root_slot = B256::from(alloy_primitives::U256::from(timestamp % 8191 + 8191));

        // EIP-2935 Block Hash History contract
        let history_addr =
            keccak256(alloy_primitives::address!("0000F90827F1C53a10cb7A02335B175320002935"));
        // Ring buffer slot: (block_number - 1) % 8191
        let history_slot =
            B256::from(alloy_primitives::U256::from((block_number.saturating_sub(1)) % 8191));

        vec![
            StorageSlotKey::new(beacon_roots_addr, beacon_ts_slot),
            StorageSlotKey::new(beacon_roots_addr, beacon_root_slot),
            StorageSlotKey::new(history_addr, history_slot),
        ]
    }
}

impl Hotness {
    /// Returns true if this account should be preserved.
    pub const fn should_preserve(&self) -> bool {
        !matches!(self, Self::Cold)
    }

    /// Returns true if storage trie should be preserved.
    pub const fn should_preserve_storage(&self) -> bool {
        matches!(self, Self::Always | Self::Likely)
    }
}

/// Tiered hot account tracking.
///
/// Tracks hot accounts using a combination of:
/// - Static known-hot addresses (system contracts, major defi)
/// - Semi-static addresses (builders, fee recipients)
/// - Dynamic detection via rotating bloom filters
#[derive(Debug)]
pub struct TieredHotAccounts {
    // === Tier A: Always hot (known contracts) ===
    /// System contract addresses (hashed).
    system_contracts: HashSet<B256>,
    /// Major defi contract addresses (hashed).
    major_contracts: HashSet<B256>,

    // === Tier B: Likely hot (semi-static) ===
    /// Known builder/searcher addresses (hashed).
    known_builders: HashSet<B256>,
    /// Recent fee recipients as a ring buffer (most recent first).
    /// Capped at `MAX_RECENT_FEE_RECIPIENTS` to prevent unbounded growth.
    recent_fee_recipients: VecDeque<B256>,
    /// Set for O(1) membership checks of recent fee recipients.
    recent_fee_recipients_set: HashSet<B256>,

    // === Tier C: Dynamic hot (runtime detected) ===
    /// Rotating bloom filter for EOAs.
    eoa_filter: RotatingBloomFilter,
    /// Rotating bloom filter for contracts (separate because they have storage).
    contract_filter: RotatingBloomFilter,
    /// Bloom filter to track which addresses are contracts.
    is_contract: BloomFilter,

    // === Configuration ===
    /// Configuration for the hot account tracker.
    #[allow(dead_code)]
    config: HotAccountConfig,
}

/// Configuration for smart pruning with hot account preservation.
#[derive(Debug)]
pub struct SmartPruneConfig<'a> {
    /// Maximum depth to keep in account trie (for non-hot accounts).
    pub max_depth: usize,
    /// Maximum number of storage tries to keep.
    pub max_storage_tries: usize,
    /// Hot account tracker.
    pub hot_accounts: &'a TieredHotAccounts,
}

impl<'a> SmartPruneConfig<'a> {
    /// Creates a new smart prune configuration.
    pub const fn new(
        max_depth: usize,
        max_storage_tries: usize,
        hot_accounts: &'a TieredHotAccounts,
    ) -> Self {
        Self { max_depth, max_storage_tries, hot_accounts }
    }
}

/// Configuration for hot account tracking.
#[derive(Debug, Clone)]
pub struct HotAccountConfig {
    /// Number of historical blocks to track.
    pub history_blocks: usize,
    /// Threshold for EOAs to be considered hot.
    pub eoa_hot_threshold: usize,
    /// Threshold for contracts to be considered hot (lower, as contracts are more valuable).
    pub contract_hot_threshold: usize,
    /// Expected accounts per block.
    pub expected_accounts_per_block: usize,
    /// Maximum memory usage for filters (bytes).
    pub max_memory_bytes: usize,
}

impl Default for HotAccountConfig {
    fn default() -> Self {
        Self {
            history_blocks: 32,
            eoa_hot_threshold: 8,
            contract_hot_threshold: 4,
            expected_accounts_per_block: 500,
            max_memory_bytes: 256 * 1024, // 256KB
        }
    }
}

impl Default for TieredHotAccounts {
    fn default() -> Self {
        Self::new(HotAccountConfig::default())
    }
}

impl TieredHotAccounts {
    /// Creates a new hot account tracker with the given configuration.
    pub fn new(config: HotAccountConfig) -> Self {
        Self {
            system_contracts: HashSet::default(),
            major_contracts: HashSet::default(),
            known_builders: HashSet::default(),
            recent_fee_recipients: VecDeque::with_capacity(MAX_RECENT_FEE_RECIPIENTS),
            recent_fee_recipients_set: HashSet::default(),
            eoa_filter: RotatingBloomFilter::new(
                config.history_blocks,
                config.eoa_hot_threshold,
                config.expected_accounts_per_block,
            ),
            contract_filter: RotatingBloomFilter::new(
                config.history_blocks,
                config.contract_hot_threshold,
                config.expected_accounts_per_block / 4, // Fewer contracts than EOAs
            ),
            is_contract: BloomFilter::with_capacity(10_000),
            config,
        }
    }

    /// Creates a tracker configured for Ethereum mainnet.
    ///
    /// Includes known system contracts and major defi protocols.
    pub fn for_mainnet() -> Self {
        use alloy_primitives::keccak256;

        let mut tracker = Self::new(HotAccountConfig::default());

        // Tier A: System contracts (addresses from EIPs)
        // EIP-4788: Beacon roots
        tracker.add_system_contract(keccak256(alloy_primitives::address!(
            "000F3df6D732807Ef1319fB7B8bB8522d0Beac02"
        )));
        // EIP-2935: Block hash history
        tracker.add_system_contract(keccak256(alloy_primitives::address!(
            "0000F90827F1C53a10cb7A02335B175320002935"
        )));
        // EIP-7002: Withdrawal requests
        tracker.add_system_contract(keccak256(alloy_primitives::address!(
            "00000961Ef480Eb55e80D19ad83579A64c007002"
        )));
        // EIP-7251: Consolidation requests
        tracker.add_system_contract(keccak256(alloy_primitives::address!(
            "0000BBdDc7CE488642fb579F8B00f3a590007251"
        )));

        // Tier A: Major defi contracts
        // WETH
        tracker.add_major_contract(keccak256(alloy_primitives::address!(
            "C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"
        )));
        // USDC
        tracker.add_major_contract(keccak256(alloy_primitives::address!(
            "A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"
        )));
        // USDT
        tracker.add_major_contract(keccak256(alloy_primitives::address!(
            "dAC17F958D2ee523a2206206994597C13D831ec7"
        )));
        // Uniswap V3 Router
        tracker.add_major_contract(keccak256(alloy_primitives::address!(
            "E592427A0AEce92De3Edee1F18E0157C05861564"
        )));
        // Uniswap Universal Router
        tracker.add_major_contract(keccak256(alloy_primitives::address!(
            "3fC91A3afd70395Cd496C647d5a6CC9D4B2b7FAD"
        )));

        tracker
    }

    /// Creates a tracker configured for Optimism.
    pub fn for_optimism() -> Self {
        use alloy_primitives::keccak256;

        let mut tracker = Self::new(HotAccountConfig::default());

        // L1 Block contract
        tracker.add_system_contract(keccak256(alloy_primitives::address!(
            "4200000000000000000000000000000000000015"
        )));
        // L2 Cross Domain Messenger
        tracker.add_system_contract(keccak256(alloy_primitives::address!(
            "4200000000000000000000000000000000000007"
        )));
        // Gas Price Oracle
        tracker.add_system_contract(keccak256(alloy_primitives::address!(
            "420000000000000000000000000000000000000F"
        )));

        tracker
    }

    /// Creates a tracker configured for Base.
    pub fn for_base() -> Self {
        // Base uses the same system contracts as Optimism
        Self::for_optimism()
    }

    /// Adds a system contract address (Tier A - always hot).
    pub fn add_system_contract(&mut self, hashed_address: B256) {
        self.system_contracts.insert(hashed_address);
        self.is_contract.insert(&hashed_address);
    }

    /// Adds a major defi contract address (Tier A - always hot).
    pub fn add_major_contract(&mut self, hashed_address: B256) {
        self.major_contracts.insert(hashed_address);
        self.is_contract.insert(&hashed_address);
    }

    /// Adds a known builder address (Tier B - likely hot).
    pub fn add_builder(&mut self, hashed_address: B256) {
        self.known_builders.insert(hashed_address);
    }

    /// Records an account as touched in the current block.
    ///
    /// # Arguments
    /// * `hashed_address` - The keccak256 hash of the account address
    /// * `has_code` - Whether the account has contract code
    pub fn record(&mut self, hashed_address: B256, has_code: bool) {
        if has_code {
            self.is_contract.insert(&hashed_address);
            self.contract_filter.insert(hashed_address);
        } else {
            self.eoa_filter.insert(hashed_address);
        }
    }

    /// Records a fee recipient for the current block.
    ///
    /// Fee recipients are automatically promoted to Tier B after being seen.
    /// The ring buffer maintains a fixed-size history to prevent unbounded growth.
    pub fn record_fee_recipient(&mut self, hashed_address: B256) {
        // Skip if already in the set
        if self.recent_fee_recipients_set.contains(&hashed_address) {
            return;
        }

        // Add to ring buffer and set
        self.recent_fee_recipients.push_front(hashed_address);
        self.recent_fee_recipients_set.insert(hashed_address);

        // Evict oldest if over capacity
        if self.recent_fee_recipients.len() > MAX_RECENT_FEE_RECIPIENTS &&
            let Some(evicted) = self.recent_fee_recipients.pop_back()
        {
            self.recent_fee_recipients_set.remove(&evicted);
        }
    }

    /// Call at the end of each block to rotate filters.
    pub fn end_block(&mut self) {
        self.eoa_filter.end_block();
        self.contract_filter.end_block();
    }

    /// Returns the hotness level of an account.
    pub fn hotness(&self, hashed_address: &B256) -> Hotness {
        // Tier A: Always hot
        if self.system_contracts.contains(hashed_address) ||
            self.major_contracts.contains(hashed_address)
        {
            return Hotness::Always;
        }

        // Tier B: Likely hot
        if self.known_builders.contains(hashed_address) ||
            self.recent_fee_recipients_set.contains(hashed_address)
        {
            return Hotness::Likely;
        }

        // Tier C: Dynamic detection
        let is_contract = self.is_contract.contains(hashed_address);
        if is_contract {
            if self.contract_filter.is_hot(hashed_address) {
                return Hotness::Dynamic;
            }
        } else if self.eoa_filter.is_hot(hashed_address) {
            return Hotness::Dynamic;
        }

        Hotness::Cold
    }

    /// Returns true if the account should be preserved in the trie.
    pub fn should_preserve(&self, hashed_address: &B256) -> bool {
        self.hotness(hashed_address).should_preserve()
    }

    /// Returns true if the account's storage trie should be preserved.
    pub fn should_preserve_storage(&self, hashed_address: &B256) -> bool {
        self.hotness(hashed_address).should_preserve_storage()
    }

    /// Handles a chain reorg by invalidating recent history.
    pub fn handle_reorg(&mut self, depth: usize) {
        self.eoa_filter.handle_reorg(depth);
        self.contract_filter.handle_reorg(depth);
    }

    /// Clears all dynamic tracking state.
    ///
    /// Static Tier A (system/major contracts) is preserved.
    /// Tier B (builders) is preserved. Recent fee recipients and dynamic filters are cleared.
    pub fn clear_dynamic(&mut self) {
        self.eoa_filter.clear();
        self.contract_filter.clear();
        self.is_contract.clear();
        self.recent_fee_recipients.clear();
        self.recent_fee_recipients_set.clear();
    }

    /// Returns approximate memory usage in bytes.
    pub fn memory_usage(&self) -> usize {
        self.eoa_filter.memory_usage() +
            self.contract_filter.memory_usage() +
            self.is_contract.len() / 8 +
            self.system_contracts.len() * 32 +
            self.major_contracts.len() * 32 +
            self.known_builders.len() * 32 +
            self.recent_fee_recipients.len() * 32 +
            self.recent_fee_recipients_set.len() * 32
    }

    /// Returns an iterator over Tier A addresses (system contracts and major contracts).
    ///
    /// These are the highest priority accounts that should always be preserved.
    pub fn tier_a_iter(&self) -> impl Iterator<Item = &B256> {
        self.system_contracts.iter().chain(self.major_contracts.iter())
    }

    /// Returns an iterator over Tier B addresses (known builders and recent fee recipients).
    ///
    /// These are semi-static accounts that are likely to be accessed.
    pub fn tier_b_iter(&self) -> impl Iterator<Item = &B256> {
        self.known_builders.iter().chain(self.recent_fee_recipients.iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;

    #[test]
    fn bloom_filter_basic() {
        let mut filter = BloomFilter::new(1024);

        let value1 = B256::repeat_byte(0x01);
        let value2 = B256::repeat_byte(0x02);
        let value3 = B256::repeat_byte(0x03);

        assert!(!filter.contains(&value1));
        assert!(!filter.contains(&value2));

        filter.insert(&value1);
        filter.insert(&value2);

        assert!(filter.contains(&value1));
        assert!(filter.contains(&value2));
        // value3 might have false positive, but likely not with a sparse filter
        let _ = filter.contains(&value3); // Just exercise the code path

        filter.clear();
        assert!(!filter.contains(&value1));
    }

    #[test]
    fn rotating_filter_hot_detection() {
        let mut filter = RotatingBloomFilter::new(10, 3, 100);

        let hot_addr = B256::repeat_byte(0xAA);
        let cold_addr = B256::repeat_byte(0xBB);

        // Insert hot_addr in multiple blocks
        for _ in 0..5 {
            filter.insert(hot_addr);
            filter.end_block();
        }

        // Insert cold_addr only once
        filter.insert(cold_addr);
        filter.end_block();

        assert!(filter.is_hot(&hot_addr));
        assert!(!filter.is_hot(&cold_addr));
    }

    #[test]
    fn tiered_hot_accounts() {
        let mut tracker = TieredHotAccounts::for_mainnet();

        // System contract should always be hot
        let system_addr = alloy_primitives::keccak256(alloy_primitives::address!(
            "000F3df6D732807Ef1319fB7B8bB8522d0Beac02"
        ));
        assert_eq!(tracker.hotness(&system_addr), Hotness::Always);

        // Random address should be cold
        let random = B256::repeat_byte(0xFF);
        assert_eq!(tracker.hotness(&random), Hotness::Cold);

        // Record address multiple times to make it hot
        for _ in 0..10 {
            tracker.record(random, false);
            tracker.end_block();
        }
        assert_eq!(tracker.hotness(&random), Hotness::Dynamic);
    }

    #[test]
    fn reorg_handling() {
        let mut filter = RotatingBloomFilter::new(10, 3, 100);

        let addr = B256::repeat_byte(0xCC);

        // Build up history
        for _ in 0..5 {
            filter.insert(addr);
            filter.end_block();
        }

        assert!(filter.is_hot(&addr));

        // Reorg removes recent history
        filter.handle_reorg(10);

        assert!(!filter.is_hot(&addr));
    }
}

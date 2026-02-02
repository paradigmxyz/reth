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
//!
//! # Example
//!
//! ```ignore
//! let mut tracker = HotAccounts::for_mainnet();
//!
//! // Record fee recipient for the block
//! tracker.record_fee_recipient(hashed_address);
//!
//! // Check if account should be preserved
//! if tracker.should_preserve(&hashed_address) {
//!     // Keep in trie
//! }
//! ```

use alloy_primitives::{address, map::HashSet, B256};
use std::collections::VecDeque;

/// Maximum number of recent fee recipients to track (prevents unbounded growth).
const MAX_RECENT_FEE_RECIPIENTS: usize = 128;

/// Tiered hot account tracking.
///
/// Tracks hot accounts using a combination of:
/// - Static known-hot addresses (system contracts, major defi)
/// - Semi-static addresses (builders, fee recipients)
#[derive(Debug, Clone)]
pub struct HotAccounts {
    /// System contract addresses (hashed) - Tier A.
    system_contracts: HashSet<B256>,
    /// Major defi contract addresses (hashed) - Tier A.
    major_contracts: HashSet<B256>,
    /// Known builder/searcher addresses (hashed) - Tier B.
    known_builders: HashSet<B256>,
    /// Recent fee recipients as a ring buffer (most recent first) - Tier B.
    /// Capped at `MAX_RECENT_FEE_RECIPIENTS` to prevent unbounded growth.
    recent_fee_recipients: VecDeque<B256>,
    /// Set for O(1) membership checks of recent fee recipients.
    recent_fee_recipients_set: HashSet<B256>,
}

impl Default for HotAccounts {
    fn default() -> Self {
        Self::new()
    }
}

impl HotAccounts {
    /// Creates a new empty hot account tracker.
    pub fn new() -> Self {
        Self {
            system_contracts: HashSet::default(),
            major_contracts: HashSet::default(),
            known_builders: HashSet::default(),
            recent_fee_recipients: VecDeque::with_capacity(MAX_RECENT_FEE_RECIPIENTS),
            recent_fee_recipients_set: HashSet::default(),
        }
    }

    /// Creates a tracker configured for any Ethereum network (mainnet, holesky, sepolia).
    ///
    /// Includes system contracts from EIPs that are common across all Ethereum networks.
    pub fn for_ethereum() -> Self {
        use alloy_primitives::keccak256;

        let mut tracker = Self::new();

        // Tier A: System contracts (addresses from EIPs - same across all Ethereum networks)
        // EIP-4788: Beacon roots
        tracker
            .add_system_contract(keccak256(address!("0x000F3df6D732807Ef1319fB7B8bB8522d0Beac02")));
        // EIP-2935: Block hash history
        tracker
            .add_system_contract(keccak256(address!("0x0000F90827F1C53a10cb7A02335B175320002935")));
        // EIP-7002: Withdrawal requests
        tracker
            .add_system_contract(keccak256(address!("0x00000961Ef480Eb55e80D19ad83579A64c007002")));
        // EIP-7251: Consolidation requests
        tracker
            .add_system_contract(keccak256(address!("0x0000BBdDc7CE488642fb579F8B00f3a590007251")));

        tracker
    }

    /// Creates a tracker configured for Ethereum mainnet.
    ///
    /// Includes system contracts and mainnet-specific defi protocols and builders.
    pub fn for_mainnet() -> Self {
        use alloy_primitives::keccak256;

        let mut tracker = Self::for_ethereum();

        // Tier A: Major defi contracts
        // WETH
        tracker
            .add_major_contract(keccak256(address!("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2")));
        // USDC
        tracker
            .add_major_contract(keccak256(address!("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48")));
        // USDT
        tracker
            .add_major_contract(keccak256(address!("0xdAC17F958D2ee523a2206206994597C13D831ec7")));
        // Uniswap V3 Router
        tracker
            .add_major_contract(keccak256(address!("0xE592427A0AEce92De3Edee1F18E0157C05861564")));
        // Uniswap Universal Router
        tracker
            .add_major_contract(keccak256(address!("0x3fC91A3afd70395Cd496C647d5a6CC9D4B2b7FAD")));

        // Tier B: Known block builders
        // Beaverbuild
        tracker.add_builder(keccak256(address!("0x95222290DD7278Aa3Ddd389Cc1E1d165CC4BAfe5")));
        // Titan Builder
        tracker.add_builder(keccak256(address!("0x4838B106FCe9647Bdf1E7877BF73cE8B0BAD5f97")));
        // Rsync
        tracker.add_builder(keccak256(address!("0x1f9090aaE28b8a3dCeaDf281B0F12828e676c326")));
        // Flashbots
        tracker.add_builder(keccak256(address!("0xDAFEA492D9c6733ae3d56b7Ed1ADb60692c98Bc5")));
        // Builder0x69
        tracker.add_builder(keccak256(address!("0x690B9A9E9aa1C9dB991C7721a92d351Db4FaC990")));
        // BuilderNet
        tracker.add_builder(keccak256(address!("0xdadb0d80178819f2319190d340ce9a924f783711")));
        // bloXroute
        tracker.add_builder(keccak256(address!("0x965Df5Ff6116C395187E288e5C87fb96CfB8141c")));
        // Lido Execution Layer Rewards Vault
        tracker.add_builder(keccak256(address!("0x388C818CA8B9251b393131C08a736A67ccB19297")));
        // Quasar
        tracker.add_builder(keccak256(address!("0x396343362be2a4da1ce0c1c210945346fb82aa49")));

        tracker
    }

    /// Creates a tracker configured for Optimism.
    pub fn for_optimism() -> Self {
        use alloy_primitives::keccak256;

        let mut tracker = Self::new();

        // L1 Block contract
        tracker
            .add_system_contract(keccak256(address!("0x4200000000000000000000000000000000000015")));
        // L2 Cross Domain Messenger
        tracker
            .add_system_contract(keccak256(address!("0x4200000000000000000000000000000000000007")));
        // Gas Price Oracle
        tracker
            .add_system_contract(keccak256(address!("0x420000000000000000000000000000000000000F")));

        tracker
    }

    /// Creates a tracker configured for Base.
    pub fn for_base() -> Self {
        // Base uses the same system contracts as Optimism
        Self::for_optimism()
    }

    /// Creates a tracker based on the chain ID.
    ///
    /// Returns the appropriate network-specific configuration:
    /// - Chain ID 1 (Ethereum mainnet): [`Self::for_mainnet`]
    /// - Chain ID 11155111 (Sepolia), 17000 (Holesky): [`Self::for_ethereum`]
    /// - Chain ID 10 (Optimism): [`Self::for_optimism`]
    /// - Chain ID 8453 (Base): [`Self::for_base`]
    /// - Other chains: Empty tracker ([`Self::new`])
    pub fn from_chain_id(chain_id: u64) -> Self {
        match chain_id {
            1 => Self::for_mainnet(),
            // Sepolia, Holesky
            11155111 | 17000 => Self::for_ethereum(),
            10 => Self::for_optimism(),
            8453 => Self::for_base(),
            _ => Self::new(),
        }
    }

    /// Adds a system contract address (Tier A - always hot).
    pub fn add_system_contract(&mut self, hashed_address: B256) {
        self.system_contracts.insert(hashed_address);
    }

    /// Adds a major defi contract address (Tier A - always hot).
    pub fn add_major_contract(&mut self, hashed_address: B256) {
        self.major_contracts.insert(hashed_address);
    }

    /// Adds a known builder address (Tier B - likely hot).
    pub fn add_builder(&mut self, hashed_address: B256) {
        self.known_builders.insert(hashed_address);
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

    /// Clears recent fee recipients.
    ///
    /// Static Tier A (system/major contracts) and Tier B (builders) are preserved.
    pub fn clear_fee_recipients(&mut self) {
        self.recent_fee_recipients.clear();
        self.recent_fee_recipients_set.clear();
    }

    /// Returns approximate memory usage in bytes.
    pub fn memory_usage(&self) -> usize {
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

/// Hotness level for an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Hotness {
    /// Cold: not tracked as hot.
    Cold = 0,
    /// Likely: known builder/fee recipient.
    Likely = 1,
    /// Always: system contract or major defi.
    Always = 2,
}

impl Hotness {
    /// Returns true if this hotness level indicates the account should be preserved.
    pub const fn should_preserve(self) -> bool {
        matches!(self, Self::Always | Self::Likely)
    }

    /// Returns true if this hotness level indicates the storage trie should be preserved.
    pub const fn should_preserve_storage(self) -> bool {
        matches!(self, Self::Always | Self::Likely)
    }
}

/// Configuration for smart pruning with hot account preservation.
#[derive(Debug)]
pub struct SmartPruneConfig<'a> {
    /// Maximum depth to keep in account trie (for non-hot accounts).
    pub max_depth: usize,
    /// Maximum number of storage tries to keep (soft limit, hot accounts can exceed).
    pub max_storage_tries: usize,
    /// Hot account tracker.
    pub hot_accounts: &'a HotAccounts,
}

impl<'a> SmartPruneConfig<'a> {
    /// Creates a new smart prune configuration.
    pub const fn new(
        max_depth: usize,
        max_storage_tries: usize,
        hot_accounts: &'a HotAccounts,
    ) -> Self {
        Self { max_depth, max_storage_tries, hot_accounts }
    }

    /// Checks if a trie path leads to a hot account that should be preserved during pruning.
    ///
    /// For complete account paths (64 nibbles), checks directly against hot accounts.
    /// For partial paths, checks if any hot account's hashed address starts with this prefix.
    pub fn path_leads_to_hot_account(&self, path: &reth_trie_common::Nibbles) -> bool {
        use alloy_primitives::B256;

        // For complete account paths (64 nibbles), check directly
        if path.len() >= 64 {
            let hashed = B256::from_slice(&path.pack()[..32]);
            return self.hot_accounts.should_preserve(&hashed);
        }

        // For partial paths, check if any known hot account has this prefix
        // Check Tier A: system contracts and major contracts
        for addr in self.hot_accounts.tier_a_iter() {
            let account_path = reth_trie_common::Nibbles::unpack(*addr);
            if account_path.starts_with(path) {
                return true;
            }
        }

        // Check Tier B: known builders and fee recipients
        for addr in self.hot_accounts.tier_b_iter() {
            let account_path = reth_trie_common::Nibbles::unpack(*addr);
            if account_path.starts_with(path) {
                return true;
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;

    #[test]
    fn tiered_hot_accounts() {
        let tracker = HotAccounts::for_mainnet();

        // System contract should always be hot
        let system_addr =
            alloy_primitives::keccak256(address!("0x000F3df6D732807Ef1319fB7B8bB8522d0Beac02"));
        assert_eq!(tracker.hotness(&system_addr), Hotness::Always);

        // Random address should be cold
        let random = B256::repeat_byte(0xFF);
        assert_eq!(tracker.hotness(&random), Hotness::Cold);
    }

    #[test]
    fn fee_recipient_tracking() {
        let mut tracker = HotAccounts::new();

        let fee_recipient = B256::repeat_byte(0xAA);
        assert_eq!(tracker.hotness(&fee_recipient), Hotness::Cold);

        tracker.record_fee_recipient(fee_recipient);
        assert_eq!(tracker.hotness(&fee_recipient), Hotness::Likely);

        tracker.clear_fee_recipients();
        assert_eq!(tracker.hotness(&fee_recipient), Hotness::Cold);
    }

    #[test]
    fn builder_tracking() {
        let tracker = HotAccounts::for_mainnet();

        // Beaverbuild should be Likely
        let beaverbuild =
            alloy_primitives::keccak256(address!("0x95222290DD7278Aa3Ddd389Cc1E1d165CC4BAfe5"));
        assert_eq!(tracker.hotness(&beaverbuild), Hotness::Likely);
    }
}

//! Provides the [`WitnessDatabase`] type, an implementation of [`reth_revm::Database`]
//! specifically designed for stateless execution environments.

use crate::trie::StatelessTrie;
use alloc::{collections::btree_map::BTreeMap, format};
use alloy_primitives::{map::B256Map, Address, B256, U256};
use reth_errors::ProviderError;
use reth_revm::{bytecode::Bytecode, state::AccountInfo, Database};

/// An EVM database implementation backed by witness data.
///
/// This struct implements the [`reth_revm::Database`] trait, allowing the EVM to execute
/// transactions using:
///  - Account and storage slot data provided by a [`StatelessTrie`].
///  - Bytecode and ancestor block hashes provided by in-memory maps.
///
/// This is designed for stateless execution scenarios where direct access to a full node's
/// database is not available or desired.
#[derive(Debug)]
pub(crate) struct WitnessDatabase<'a> {
    /// Map of block numbers to block hashes.
    /// This is used to service the `BLOCKHASH` opcode.
    // TODO: use Vec instead -- ancestors should be contiguous
    // TODO: so we can use the current_block_number and an offset to
    // TODO: get the block number of a particular ancestor
    block_hashes_by_block_number: BTreeMap<u64, B256>,
    /// Map of code hashes to bytecode.
    /// Used to fetch contract code needed during execution.
    bytecode: B256Map<Bytecode>,
    /// The sparse Merkle Patricia Trie containing account and storage state.
    /// This is used to provide account/storage values during EVM execution.
    /// TODO: Ideally we do not have this trie and instead a simple map.
    /// TODO: Then as a corollary we can avoid unnecessary hashing in `Database::storage`
    /// TODO: and `Database::basic` without needing to cache the hashed Addresses and Keys
    trie: &'a StatelessTrie,
}

impl<'a> WitnessDatabase<'a> {
    /// Creates a new [`WitnessDatabase`] instance.
    ///
    /// # Assumptions
    ///
    /// This function assumes:
    /// 1. The provided `trie` has been populated with state data consistent with a known state root
    ///    (e.g., using witness data and verifying against a parent block's state root).
    /// 2. The `bytecode` map contains all bytecode corresponding to code hashes present in the
    ///    account data within the `trie`.
    /// 3. The `ancestor_hashes` map contains the block hashes for the relevant ancestor blocks (up
    ///    to 256 including the current block number). It assumes these hashes correspond to a
    ///    contiguous chain of blocks. The caller is responsible for verifying the contiguity and
    ///    the block limit.
    pub(crate) const fn new(
        trie: &'a StatelessTrie,
        bytecode: B256Map<Bytecode>,
        ancestor_hashes: BTreeMap<u64, B256>,
    ) -> Self {
        Self { trie, block_hashes_by_block_number: ancestor_hashes, bytecode }
    }
}

impl Database for WitnessDatabase<'_> {
    /// The database error type.
    type Error = ProviderError;

    /// Get basic account information by hashing the address and looking up the account RLP
    /// in the underlying [`StatelessTrie`].
    ///
    /// Returns `Ok(None)` if the account is not found in the trie.
    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        if let Some(account) = self.trie.account(address)? {
            return Ok(Some(AccountInfo {
                balance: account.balance,
                nonce: account.nonce,
                code_hash: account.code_hash,
                code: None,
            }));
        };

        Ok(None)
    }

    /// Get storage value of an account at a specific slot.
    ///
    /// Returns `U256::ZERO` if the slot is not found in the trie.
    fn storage(&mut self, address: Address, slot: U256) -> Result<U256, Self::Error> {
        self.trie.storage(address, slot)
    }

    /// Get account code by its hash from the provided bytecode map.
    ///
    /// Returns an error if the bytecode for the given hash is not found in the map.
    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        let bytecode = self.bytecode.get(&code_hash).ok_or_else(|| {
            ProviderError::TrieWitnessError(format!("bytecode for {code_hash} not found"))
        })?;

        Ok(bytecode.clone())
    }

    /// Get block hash by block number from the provided ancestor hashes map.
    ///
    /// Returns an error if the hash for the given block number is not found in the map.
    fn block_hash(&mut self, block_number: u64) -> Result<B256, Self::Error> {
        self.block_hashes_by_block_number
            .get(&block_number)
            .copied()
            .ok_or(ProviderError::StateForNumberNotFound(block_number))
    }
}

//! Provides the [`WitnessDatabase`] type, an implementation of [`reth_revm::Database`]
//! specifically designed for stateless execution environments.

use alloy_primitives::{keccak256, map::B256Map, Address, B256, U256};
use alloy_rlp::Decodable;
use alloy_trie::TrieAccount;
use reth_errors::ProviderError;
use reth_revm::{bytecode::Bytecode, state::AccountInfo, Database};
use reth_trie_sparse::SparseStateTrie;
use std::collections::HashMap;
use tracing::trace;

/// An EVM database implementation backed by witness data.
///
/// This struct implements the [`reth_revm::Database`] trait, allowing the EVM to execute
/// transactions using:
///  - Account and storage slot data provided by a [`reth_trie_sparse::SparseStateTrie`].
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
    block_hashes_by_block_number: HashMap<u64, B256>,
    /// Map of code hashes to bytecode.
    /// Used to fetch contract code needed during execution.
    bytecode: B256Map<Bytecode>,
    /// The sparse Merkle Patricia Trie containing account and storage state.
    /// This is used to provide account/storage values during EVM execution.
    /// TODO: Ideally we do not have this trie and instead a simple map.
    /// TODO: Then as a corollary we can avoid unnecessary hashing in `Database::storage`
    /// TODO: and `Database::basic` without needing to cache the hashed Addresses and Keys
    trie: &'a SparseStateTrie,
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
        trie: &'a SparseStateTrie,
        bytecode: B256Map<Bytecode>,
        ancestor_hashes: HashMap<u64, B256>,
    ) -> Self {
        Self { trie, block_hashes_by_block_number: ancestor_hashes, bytecode }
    }
}

impl Database for WitnessDatabase<'_> {
    /// The database error type.
    type Error = ProviderError;

    /// Get basic account information by hashing the address and looking up the account RLP
    /// in the underlying [`SparseStateTrie`].
    ///
    /// Returns `Ok(None)` if the account is not found in the trie.
    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        let hashed_address = keccak256(address);
        trace!(target: "reth-stateless::evm", %address, %hashed_address, "retrieving account");
        let Some(bytes) = self.trie.get_account_value(&hashed_address) else {
            trace!(target: "reth-stateless::evm", %address, %hashed_address, "no account found");
            return Ok(None)
        };
        let account = TrieAccount::decode(&mut bytes.as_slice())?;
        let account_info = AccountInfo {
            balance: account.balance,
            nonce: account.nonce,
            code_hash: account.code_hash,
            code: None,
        };
        trace!(target: "reth-stateless::evm", %address, %hashed_address, ?account_info, "account retrieved");
        Ok(Some(account_info))
    }

    /// Get storage value of an account at a specific slot.
    ///
    ///  Returns `U256::ZERO` if the slot is not found in the trie.
    fn storage(&mut self, address: Address, slot: U256) -> Result<U256, Self::Error> {
        let slot = B256::from(slot);
        let hashed_address = keccak256(address);
        let hashed_slot = keccak256(slot);
        trace!(target: "reth-stateless::evm", %address, %hashed_address, %slot, %hashed_slot, "retrieving storage slot");
        let value =
            if let Some(value) = self.trie.get_storage_slot_value(&hashed_address, &hashed_slot) {
                U256::decode(&mut value.as_slice())?
            } else {
                U256::ZERO
            };
        trace!(target: "reth-stateless::evm", %address, %hashed_address, %slot, %hashed_slot, %value, "storage slot retrieved");
        Ok(value)
    }

    /// Get account code by its hash from the provided bytecode map.
    ///
    /// Returns an error if the bytecode for the given hash is not found in the map.
    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        trace!(target: "reth-stateless::evm", %code_hash, "retrieving bytecode");

        // TODO: This was added because the beacon root contract was not being included
        // TODO: in the witness, when there were no transactions. See `notxs.json` in
        // TODO: the execution spec tests. Once it is fixed, we can remove this.
        use alloy_eips::eip4788::BEACON_ROOTS_CODE;
        let beacon_root_hash = keccak256(BEACON_ROOTS_CODE.clone());
        if code_hash == beacon_root_hash {
            return Ok(Bytecode::new_raw(BEACON_ROOTS_CODE.clone()));
        }

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

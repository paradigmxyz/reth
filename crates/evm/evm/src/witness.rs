use alloc::vec::Vec;
use alloy_primitives::{keccak256, Bytes, B256};
use alloy_rpc_types_debug::ExecutionWitness;
use evm2::evm::CacheDB;
use reth_primitives_traits::Account as PrimitiveAccount;
use reth_storage_api::{
    AccountReader, HashedPostStateProvider, HeaderProvider, StateProofProvider,
};
use reth_storage_errors::provider::ProviderResult;
use reth_trie_common::{ExecutionWitnessMode, HashedPostState};

/// Borrows finalized execution state for witness generation.
#[derive(Debug, Clone)]
pub struct ExecutionWitnessRecord<'a, DB> {
    /// State after execution.
    state: &'a CacheDB<DB>,
    /// Additional hashed state to include in the witness.
    additional_state: Option<HashedPostState>,
    /// Code read from parent state, before execution output was committed.
    canonical_codes: Option<Vec<Bytes>>,
}

impl<'a, DB> ExecutionWitnessRecord<'a, DB> {
    /// Creates a new record from the state after execution.
    pub const fn new(state: &'a CacheDB<DB>) -> Self {
        Self { state, additional_state: None, canonical_codes: None }
    }

    /// Adds hashed state that should be included when generating the witness.
    ///
    /// State recorded during execution takes precedence over additional state for overlapping
    /// accounts and storage slots.
    pub fn with_additional_state(mut self, additional_state: HashedPostState) -> Self {
        self.additional_state.get_or_insert_default().extend(additional_state);
        self
    }

    /// Converts this record into a complete [`ExecutionWitness`] by generating state proofs and
    /// fetching ancestor block headers.
    ///
    /// The `block_number` is the number of the block being witnessed. Ancestor headers are
    /// included based on the lowest block number referenced by BLOCKHASH opcodes during
    /// execution, or just the parent header if BLOCKHASH was not called.
    pub fn into_execution_witness<SP, HP>(
        self,
        state_provider: &SP,
        headers_provider: &HP,
        block_number: u64,
        mode: ExecutionWitnessMode,
    ) -> ProviderResult<ExecutionWitness>
    where
        SP: StateProofProvider + HashedPostStateProvider + AccountReader + ?Sized,
        HP: HeaderProvider + ?Sized,
        HP::Header: alloy_rlp::Encodable,
    {
        let lowest_block_number =
            self.state.cache.block_hashes.keys().map(|number| number.saturating_to::<u64>()).min();
        let mut exec_witness = self.into_execution_witness_without_headers(state_provider, mode)?;

        let smallest = lowest_block_number.unwrap_or_else(|| block_number.saturating_sub(1));
        exec_witness.headers = headers_provider
            .headers_range(smallest..block_number)?
            .into_iter()
            .map(|header| {
                let mut buf = Vec::new();
                alloy_rlp::Encodable::encode(&header, &mut buf);
                buf.into()
            })
            .collect();

        Ok(exec_witness)
    }

    /// Generates state proofs and codes without fetching ancestor headers.
    ///
    /// Callers witnessing non-canonical blocks can supply headers by following parent hashes.
    pub fn into_execution_witness_without_headers<SP>(
        mut self,
        state_provider: &SP,
        mode: ExecutionWitnessMode,
    ) -> ProviderResult<ExecutionWitness>
    where
        SP: StateProofProvider + HashedPostStateProvider + AccountReader + ?Sized,
    {
        let codes = if mode.is_canonical() {
            self.canonical_codes.take().unwrap_or_else(|| Self::capture_codes(self.state))
        } else {
            self.state
                .cache
                .contracts
                .iter()
                .filter(|(hash, _)| !hash.is_zero())
                .map(|(_, code)| code.original_bytes())
                .collect()
        };

        let mut wiped_state = revm::database::BundleState::default();
        for (address, storage) in &self.state.cache.storage {
            if storage.wiped {
                let info = self
                    .state
                    .cache
                    .accounts
                    .get(address)
                    .and_then(|info| info.as_ref())
                    .map(reth_execution_types::revm_account);
                wiped_state.state.insert(
                    *address,
                    revm::database::BundleAccount::new(
                        // Original existence determines whether parent storage proofs are needed.
                        state_provider.basic_account(address)?.map(Into::into),
                        info.clone(),
                        Default::default(),
                        if info.is_some() {
                            revm::database::AccountStatus::DestroyedChanged
                        } else {
                            revm::database::AccountStatus::Destroyed
                        },
                    ),
                );
            }
        }
        if !wiped_state.is_empty() {
            self.additional_state
                .get_or_insert_default()
                .extend(state_provider.hashed_post_state(&wiped_state)?);
        }
        let (hashed_state, keys) = self.hashed_post_state();
        let state = state_provider.witness(Default::default(), hashed_state, mode)?;
        Ok(ExecutionWitness { state, codes, keys, ..Default::default() })
    }

    /// Captures canonical witness bytecode after execution, before committing its output.
    ///
    /// The cache must contain only parent-state reads, excluding code produced by execution.
    pub fn capture_codes(state: &CacheDB<DB>) -> Vec<Bytes> {
        let mut codes = state
            .cache
            .contracts
            .iter()
            .filter(|(hash, code)| !hash.is_zero() && !code.is_empty())
            .map(|(_, code)| code.original_bytes())
            .collect::<Vec<_>>();
        codes.sort_unstable();
        codes
    }

    /// Uses captured parent-state bytecode for canonical witnesses, retaining all code in legacy
    /// mode.
    pub fn with_canonical_codes(mut self, codes: Vec<Bytes>) -> Self {
        self.canonical_codes = Some(codes);
        self
    }

    fn hashed_post_state(self) -> (HashedPostState, Vec<Bytes>) {
        let mut hashed_state = self.additional_state.unwrap_or_default();
        let mut keys = Vec::new();
        for (address, account) in &self.state.cache.accounts {
            let hashed_address = keccak256(address);
            hashed_state.accounts.insert(
                hashed_address,
                account.as_ref().map(|account| PrimitiveAccount {
                    nonce: account.nonce,
                    balance: account.balance,
                    bytecode_hash: (!account.code_hash.is_zero() &&
                        account.code_hash != alloy_consensus::constants::KECCAK_EMPTY)
                        .then_some(account.code_hash),
                    #[cfg(feature = "account-ext")]
                    extension: reth_primitives_traits::AccountExtension::from_shared(
                        account.extension.clone().into_shared(),
                    ),
                }),
            );
            if account.is_some() {
                keys.push(address.to_vec().into());
            }

            if let Some(storage) = self.state.cache.storage.get(address) {
                let hashed_storage = hashed_state.storages.entry(hashed_address).or_default();
                for (slot, value) in &storage.slots {
                    let slot = B256::from(*slot);
                    hashed_storage.storage.insert(keccak256(slot), *value);
                    keys.push(slot.into());
                }
            }
        }
        (hashed_state, keys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, U256};
    use evm2::evm::{AccountInfo, EmptyDB};
    use reth_trie_common::HashedStorage;

    #[test]
    fn destroyed_account_is_recorded_for_witness() {
        let address = Address::with_last_byte(1);
        let hashed_address = keccak256(address);

        let mut state = CacheDB::<EmptyDB>::default();
        state.cache.accounts.insert(address, None);
        state.cache.storage.entry(address).or_default().wipe();

        let (hashed_state, _) = ExecutionWitnessRecord::new(&state).hashed_post_state();
        assert_eq!(hashed_state.accounts[&hashed_address], None);
        assert!(hashed_state.storages[&hashed_address].storage.is_empty());
    }

    #[cfg(feature = "account-ext")]
    #[test]
    fn witness_preserves_account_extension() {
        let address = Address::with_last_byte(1);
        let mut state = CacheDB::<EmptyDB>::default();
        state.insert_account_info(
            &address,
            AccountInfo {
                extension: evm2::evm::AccountExtension::copy_from_slice(b"account extension"),
                ..Default::default()
            },
        );
        let (hashed, _) = ExecutionWitnessRecord::new(&state).hashed_post_state();
        assert_eq!(
            hashed.accounts[&keccak256(address)].as_ref().unwrap().extension.as_ref(),
            b"account extension"
        );
    }

    #[test]
    fn additional_state_is_merged_with_executed_state() {
        let address = Address::with_last_byte(1);
        let hashed_address = keccak256(address);
        let slot = U256::from(1);
        let additional_slot = B256::with_last_byte(2);

        let mut state = CacheDB::<EmptyDB>::default();
        state.insert_account_info(&address, AccountInfo::default());
        state.insert_account_storage(&address, &slot, &U256::from(2));

        let additional_state = HashedPostState::default().with_storages([(
            hashed_address,
            HashedStorage::from_iter([
                (keccak256(B256::from(slot)), U256::from(1)),
                (additional_slot, U256::from(3)),
            ]),
        )]);

        let (hashed_state, _) = ExecutionWitnessRecord::new(&state)
            .with_additional_state(additional_state)
            .hashed_post_state();
        let storage = &hashed_state.storages[&hashed_address].storage;
        assert_eq!(storage[&keccak256(B256::from(slot))], U256::from(2));
        assert_eq!(storage[&additional_slot], U256::from(3));
    }
}

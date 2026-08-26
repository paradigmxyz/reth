use alloc::vec::Vec;
use alloy_primitives::{keccak256, map::HashMap, Address, Bytes, B256, U256};
use reth_trie::{ExecutionWitnessMode, HashedPostState, HashedStorage};
use revm::database::State;

/// Accounts and storage slots (with their post-execution values) observed during execution,
/// keyed by address.
///
/// Collected from the per-transaction journal states, which see accesses that never reach
/// the post-execution cache: storage reads of accounts created in the same transaction
/// (e.g. EIP-7702 delegations reading their uninitialized storage) and accounts that end
/// execution non-existing.
pub type AccessedState = HashMap<Address, HashMap<B256, U256>>;

/// Borrows finalized execution state for witness generation.
#[derive(Debug, Clone, Copy)]
pub struct ExecutionWitnessRecord<'a, DB> {
    /// State after execution.
    state: &'a State<DB>,
    /// Accesses observed by execution, used to backfill key preimages and proof targets
    /// that the cache alone cannot provide.
    accessed: Option<&'a AccessedState>,
}

impl<'a, DB> ExecutionWitnessRecord<'a, DB> {
    /// Creates a new record from the state after execution.
    pub const fn new(state: &'a State<DB>) -> Self {
        Self { state, accessed: None }
    }

    /// Attaches accesses observed during execution (see [`AccessedState`]).
    pub const fn with_accessed(mut self, accessed: &'a AccessedState) -> Self {
        self.accessed = Some(accessed);
        self
    }

    /// Converts this record into a complete [`alloy_rpc_types_debug::ExecutionWitness`] by
    /// generating state proofs and fetching ancestor block headers.
    ///
    /// The `block_number` is the number of the block being witnessed. Ancestor headers are
    /// included based on the lowest block number referenced by BLOCKHASH opcodes during
    /// execution, or just the parent header if BLOCKHASH was not called.
    #[cfg(feature = "witness")]
    pub fn into_execution_witness<SP, HP>(
        self,
        state_provider: &SP,
        headers_provider: &HP,
        block_number: u64,
        mode: ExecutionWitnessMode,
    ) -> reth_storage_errors::provider::ProviderResult<alloy_rpc_types_debug::ExecutionWitness>
    where
        SP: reth_storage_api::HashedPostStateProvider
            + reth_storage_api::StateProofProvider
            + ?Sized,
        HP: reth_storage_api::HeaderProvider + ?Sized,
        HP::Header: alloy_rlp::Encodable,
    {
        let codes = match mode {
            ExecutionWitnessMode::Legacy => self
                .state
                .cache
                .contracts
                .values()
                .map(|code| code.original_bytes())
                .chain(
                    // cache state does not have all the contracts, especially when
                    // a contract is created within the block
                    // the contract only exists in bundle state, therefore we need
                    // to include them as well
                    self.state.bundle_state.contracts.values().map(|code| code.original_bytes()),
                )
                .collect(),
            ExecutionWitnessMode::Canonical => {
                let mut codes: Vec<_> = self
                    .state
                    .cache
                    .contracts
                    .values()
                    .map(|c| c.original_bytes())
                    .filter(|code| !code.is_empty())
                    .collect();
                codes.sort_unstable();
                codes
            }
        };

        let (hashed_state, keys) = self.hashed_post_state(state_provider)?;

        let state = state_provider.witness(Default::default(), hashed_state, mode)?;
        let mut exec_witness =
            alloy_rpc_types_debug::ExecutionWitness { state, codes, keys, ..Default::default() };

        let lowest_block_number =
            self.state.block_hashes.lowest().map(|(block_number, _)| block_number);
        let smallest = lowest_block_number.unwrap_or_else(|| block_number.saturating_sub(1));
        let range = smallest..block_number;

        exec_witness.headers = headers_provider
            .headers_range(range)?
            .into_iter()
            .map(|header| {
                let mut buf = Vec::new();
                alloy_rlp::Encodable::encode(&header, &mut buf);
                buf.into()
            })
            .collect();

        Ok(exec_witness)
    }

    #[cfg(feature = "witness")]
    fn hashed_post_state<SP>(
        &self,
        state_provider: &SP,
    ) -> reth_storage_errors::provider::ProviderResult<(HashedPostState, Vec<Bytes>)>
    where
        SP: reth_storage_api::HashedPostStateProvider + ?Sized,
    {
        let mut hashed_state = HashedPostState::default();
        let mut keys = Vec::new();
        for (address, account) in &self.state.cache.accounts {
            let hashed_address = keccak256(address);
            hashed_state
                .accounts
                .insert(hashed_address, account.account.as_ref().map(|a| (&a.info).into()));

            let storage = hashed_state
                .storages
                .entry(hashed_address)
                .or_insert_with(|| HashedStorage::new(false));

            // The account preimage must be recorded even when the account no longer
            // exists post-execution (destroyed, or emptied by EIP-161 state clear):
            // its presence in the cache means execution accessed it, and a stateless
            // consumer needs the address preimage to resolve the pre-state leaf.
            keys.push(address.to_vec().into());

            if let Some(account) = &account.account {
                for (slot, value) in &account.storage {
                    let slot = B256::from(*slot);
                    let hashed_slot = keccak256(slot);
                    storage.storage.insert(hashed_slot, *value);

                    keys.push(slot.into());
                }
            }
        }

        // Backfill accesses the cache cannot see: reads against accounts that were
        // journal-resident when accessed (created in the same transaction) or that ended
        // execution non-existing leave no trace in `cache.accounts[..].account`, so their
        // key preimages and proof targets must come from the recorded journal states.
        if let Some(accessed) = self.accessed {
            for (address, slots) in accessed {
                let hashed_address = keccak256(address);
                let cached_account =
                    self.state.cache.accounts.get(address).and_then(|a| a.account.as_ref());

                if cached_account.is_none() {
                    hashed_state.accounts.entry(hashed_address).or_insert(None);
                    keys.push(address.to_vec().into());
                }

                let storage = hashed_state
                    .storages
                    .entry(hashed_address)
                    .or_insert_with(|| HashedStorage::new(false));
                for (slot, value) in slots {
                    let in_cache = cached_account.is_some_and(|account| {
                        account.storage.contains_key(&U256::from_be_bytes(slot.0))
                    });
                    if !in_cache {
                        storage.storage.insert(keccak256(slot), *value);
                        keys.push((*slot).into());
                    }
                }
            }
        }

        // The execution cache does not contain untouched slots of a destroyed account. The
        // provider expands them into explicit zero writes from the parent state; extending it last
        // also ensures the bundle's final values override those collected from the cache.
        hashed_state.extend(state_provider.hashed_post_state(&self.state.bundle_state)?);
        Ok((hashed_state, keys))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, U256};
    use reth_storage_api::HashedPostStateProvider;
    use reth_storage_errors::provider::ProviderResult;
    use revm::{
        database::{states::CacheAccount, AccountStatus, BundleAccount, EmptyDB},
        state::AccountInfo,
    };

    #[derive(Debug)]
    struct ExpandedStateProvider(HashedPostState);

    impl HashedPostStateProvider for ExpandedStateProvider {
        fn hashed_post_state(
            &self,
            bundle_state: &revm::database::BundleState,
        ) -> ProviderResult<HashedPostState> {
            assert!(bundle_state.state.values().any(BundleAccount::was_destroyed));
            Ok(self.0.clone())
        }
    }

    #[derive(Debug)]
    struct EmptyStateProvider;

    impl HashedPostStateProvider for EmptyStateProvider {
        fn hashed_post_state(
            &self,
            _bundle_state: &revm::database::BundleState,
        ) -> ProviderResult<HashedPostState> {
            Ok(HashedPostState::default())
        }
    }

    #[test]
    fn accessed_state_backfills_missing_preimages() {
        // Cached account whose slot read never reached the cache (journal-resident at access
        // time), plus an account execution touched that ended non-existing.
        let cached_addr = Address::with_last_byte(1);
        let journal_only_addr = Address::with_last_byte(2);
        let slot = B256::with_last_byte(3);
        let value = U256::from(7);

        let mut state = State::builder().with_database(EmptyDB::default()).build();
        state.cache.accounts.insert(
            cached_addr,
            CacheAccount::new_loaded(AccountInfo::default(), Default::default()),
        );

        let mut accessed = AccessedState::default();
        accessed.entry(cached_addr).or_default().insert(slot, value);
        accessed.entry(journal_only_addr).or_default();

        let (hashed_state, keys) = ExecutionWitnessRecord::new(&state)
            .with_accessed(&accessed)
            .hashed_post_state(&EmptyStateProvider)
            .unwrap();

        let has_key = |preimage: &[u8]| keys.iter().any(|k| k.as_ref() == preimage);
        assert!(has_key(slot.as_slice()), "missing slot preimage from accessed state");
        assert!(
            has_key(journal_only_addr.as_slice()),
            "missing address preimage for journal-only account"
        );

        let hashed_addr = keccak256(cached_addr);
        let storage = hashed_state.storages.get(&hashed_addr).unwrap();
        assert_eq!(storage.storage.get(&keccak256(slot)), Some(&value));
        assert_eq!(
            hashed_state.accounts.get(&keccak256(journal_only_addr)),
            Some(&None),
            "journal-only account must become an exclusion proof target"
        );
    }

    #[test]
    fn destroyed_account_storage_is_zero_expanded_without_wipe() {
        let address = Address::with_last_byte(1);
        let hashed_address = keccak256(address);
        let hashed_slot = B256::with_last_byte(2);

        let mut state = State::builder().with_database(EmptyDB::default()).build();
        state.cache.accounts.insert(address, CacheAccount::new_destroyed());
        state.bundle_state.state.insert(
            address,
            BundleAccount::new(
                Some(AccountInfo::default()),
                None,
                Default::default(),
                AccountStatus::Destroyed,
            ),
        );

        let provider = ExpandedStateProvider(
            HashedPostState::default().with_accounts([(hashed_address, None)]).with_storages([(
                hashed_address,
                HashedStorage::from_iter([(hashed_slot, U256::ZERO)]),
            )]),
        );

        let (hashed_state, _) =
            ExecutionWitnessRecord::new(&state).hashed_post_state(&provider).unwrap();
        let storage = hashed_state.storages.get(&hashed_address).unwrap();
        assert!(!storage.wiped);
        assert_eq!(storage.storage.get(&hashed_slot), Some(&U256::ZERO));
    }
}

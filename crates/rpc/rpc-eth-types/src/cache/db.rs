//! Helper types to workaround 'higher-ranked lifetime error'
//! <https://github.com/rust-lang/rust/issues/100013> in default implementation of
//! `reth_rpc_eth_api::helpers::Call`.

use crate::error::StateOverrideError;
use alloy_primitives::{Address, B256, U256};
use alloy_rpc_types_eth::{state::StateOverride, BlockOverrides};
use evm2::{
    bytecode::Bytecode,
    evm::{CacheDB, Db, DynDatabase},
};
use reth_errors::ProviderResult;
use reth_evm::{database::StateProviderDatabase, EvmEnv};
use reth_execution_types::EvmState;
use reth_storage_api::{BytecodeReader, HashedPostStateProvider, StateProvider, StateProviderBox};
use reth_trie::{HashedPostState, HashedStorage, MultiProofTargets};

/// Helper alias type for cached state access.
pub type StateCacheDb = CacheDB<Db<StateProviderDatabase<StateProviderTraitObjWrapper>>>;

/// Applies RPC block overrides to an evm2 environment and state cache.
pub fn apply_block_overrides<DB>(
    overrides: BlockOverrides,
    db: &mut CacheDB<DB>,
    evm_env: &mut impl EvmEnv,
) {
    let BlockOverrides {
        number,
        difficulty,
        time,
        gas_limit,
        coinbase,
        random,
        base_fee,
        blob_base_fee,
        block_hash,
        ..
    } = overrides;

    if let Some(block_hashes) = block_hash {
        for (number, hash) in block_hashes {
            db.insert_block_hash(&U256::from(number), &hash);
        }
    }

    let block = evm_env.block_env_mut();
    if let Some(number) = number {
        block.number = number;
    }
    if let Some(difficulty) = difficulty {
        block.difficulty = difficulty;
    }
    if let Some(time) = time {
        block.timestamp = U256::from(time);
    }
    if let Some(gas_limit) = gas_limit {
        block.gas_limit = U256::from(gas_limit);
    }
    if let Some(coinbase) = coinbase {
        block.beneficiary = coinbase;
    }
    if let Some(random) = random {
        block.prevrandao = U256::from_be_slice(random.as_slice());
    }
    if let Some(base_fee) = base_fee {
        block.basefee = U256::from(base_fee);
    }
    if let Some(blob_base_fee) = blob_base_fee {
        block.blob_basefee = U256::from(blob_base_fee);
    }
}

/// Applies RPC state overrides to an evm2 state cache.
pub fn apply_state_overrides<DB: DynDatabase>(
    overrides: StateOverride,
    db: &mut CacheDB<DB>,
) -> Result<(), StateOverrideError<evm2::AnyError>> {
    for (address, account_override) in overrides {
        let mut account = db
            .get_account(&address)
            .map_err(|code| StateOverrideError::Database(db.error(code)))?
            .unwrap_or_default();

        if let Some(nonce) = account_override.nonce {
            account.nonce = nonce;
        }
        if let Some(code) = account_override.code {
            let code = Bytecode::new_raw_checked(code)?;
            account.code_hash = code.hash_slow();
            account.code = Some(code);
        }
        if let Some(balance) = account_override.balance {
            account.balance = balance;
        }

        let storage = match (account_override.state, account_override.state_diff) {
            (Some(_), Some(_)) => return Err(StateOverrideError::BothStateAndStateDiff(address)),
            (Some(state), None) => {
                db.cache.storage.entry(address).or_default().wipe();
                Some(state)
            }
            (None, Some(state)) => Some(state),
            (None, None) => None,
        };

        db.insert_account_info(&address, account);
        if let Some(storage) = storage {
            for (key, value) in storage {
                db.insert_account_storage(
                    &address,
                    &U256::from_be_bytes(key.0),
                    &U256::from_be_bytes(value.0),
                );
            }
        }
    }

    Ok(())
}

/// Hack to get around 'higher-ranked lifetime error', see
/// <https://github.com/rust-lang/rust/issues/100013>
///
/// Apparently, when dealing with our RPC code, compiler is struggling to prove lifetimes around
/// [`StateProvider`] trait objects. This type is a workaround which should help the compiler to
/// understand that there are no lifetimes involved.
#[expect(missing_debug_implementations)]
pub struct StateProviderTraitObjWrapper(pub StateProviderBox);

impl reth_storage_api::StateRootProvider for StateProviderTraitObjWrapper {
    fn state_root(
        &self,
        hashed_state: reth_trie::HashedPostState,
    ) -> reth_errors::ProviderResult<B256> {
        self.0.state_root(hashed_state)
    }

    fn state_root_from_nodes(
        &self,
        input: reth_trie::TrieInput,
    ) -> reth_errors::ProviderResult<B256> {
        self.0.state_root_from_nodes(input)
    }

    fn state_root_with_updates(
        &self,
        hashed_state: reth_trie::HashedPostState,
    ) -> reth_errors::ProviderResult<(B256, reth_trie::updates::TrieUpdates)> {
        self.0.state_root_with_updates(hashed_state)
    }

    fn state_root_from_nodes_with_updates(
        &self,
        input: reth_trie::TrieInput,
    ) -> reth_errors::ProviderResult<(B256, reth_trie::updates::TrieUpdates)> {
        self.0.state_root_from_nodes_with_updates(input)
    }
}

impl reth_storage_api::StorageRootProvider for StateProviderTraitObjWrapper {
    fn storage_root(
        &self,
        address: Address,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<B256> {
        self.0.storage_root(address, hashed_storage)
    }

    fn storage_proof(
        &self,
        address: Address,
        slot: B256,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<reth_trie::StorageProof> {
        self.0.storage_proof(address, slot, hashed_storage)
    }

    fn storage_multiproof(
        &self,
        address: Address,
        slots: &[B256],
        hashed_storage: HashedStorage,
    ) -> ProviderResult<reth_trie::StorageMultiProof> {
        self.0.storage_multiproof(address, slots, hashed_storage)
    }
}

impl reth_storage_api::StateProofProvider for StateProviderTraitObjWrapper {
    fn proof(
        &self,
        input: reth_trie::TrieInput,
        address: Address,
        slots: &[B256],
    ) -> reth_errors::ProviderResult<reth_trie::AccountProof> {
        self.0.proof(input, address, slots)
    }

    fn multiproof(
        &self,
        input: reth_trie::TrieInput,
        targets: MultiProofTargets,
    ) -> ProviderResult<reth_trie::MultiProof> {
        self.0.multiproof(input, targets)
    }

    fn witness(
        &self,
        input: reth_trie::TrieInput,
        target: reth_trie::HashedPostState,
        mode: reth_trie::ExecutionWitnessMode,
    ) -> reth_errors::ProviderResult<Vec<alloy_primitives::Bytes>> {
        self.0.witness(input, target, mode)
    }
}

impl reth_storage_api::AccountReader for StateProviderTraitObjWrapper {
    fn basic_account(
        &self,
        address: &Address,
    ) -> reth_errors::ProviderResult<Option<reth_primitives_traits::Account>> {
        self.0.basic_account(address)
    }
}

impl reth_storage_api::BlockHashReader for StateProviderTraitObjWrapper {
    fn block_hash(
        &self,
        block_number: alloy_primitives::BlockNumber,
    ) -> reth_errors::ProviderResult<Option<B256>> {
        self.0.block_hash(block_number)
    }

    fn convert_block_hash(
        &self,
        hash_or_number: alloy_rpc_types_eth::BlockHashOrNumber,
    ) -> reth_errors::ProviderResult<Option<B256>> {
        self.0.convert_block_hash(hash_or_number)
    }

    fn canonical_hashes_range(
        &self,
        start: alloy_primitives::BlockNumber,
        end: alloy_primitives::BlockNumber,
    ) -> reth_errors::ProviderResult<Vec<B256>> {
        self.0.canonical_hashes_range(start, end)
    }
}

impl HashedPostStateProvider for StateProviderTraitObjWrapper {
    fn hashed_post_state(&self, state: &EvmState) -> HashedPostState {
        self.0.hashed_post_state(state)
    }
}

impl StateProvider for StateProviderTraitObjWrapper {
    fn storage(
        &self,
        account: Address,
        storage_key: alloy_primitives::StorageKey,
    ) -> reth_errors::ProviderResult<Option<alloy_primitives::StorageValue>> {
        self.0.storage(account, storage_key)
    }

    fn account_code(
        &self,
        addr: &Address,
    ) -> reth_errors::ProviderResult<Option<reth_primitives_traits::Bytecode>> {
        self.0.account_code(addr)
    }

    fn account_balance(&self, addr: &Address) -> reth_errors::ProviderResult<Option<U256>> {
        self.0.account_balance(addr)
    }

    fn account_nonce(&self, addr: &Address) -> reth_errors::ProviderResult<Option<u64>> {
        self.0.account_nonce(addr)
    }
}

impl BytecodeReader for StateProviderTraitObjWrapper {
    fn bytecode_by_hash(
        &self,
        code_hash: &B256,
    ) -> reth_errors::ProviderResult<Option<reth_primitives_traits::Bytecode>> {
        self.0.bytecode_by_hash(code_hash)
    }
}

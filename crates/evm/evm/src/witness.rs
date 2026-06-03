use crate::database::State;
use alloc::vec::Vec;
use alloy_primitives::{keccak256, Bytes, B256};
use reth_trie::{ExecutionWitnessMode, HashedPostState, HashedStorage};

/// Tracks state changes during execution.
#[derive(Debug, Clone, Default)]
pub struct ExecutionWitnessRecord {
    /// Records all state changes.
    pub hashed_state: HashedPostState,
    /// Contract code preimages required during execution and state root recomputation.
    pub codes: Vec<Bytes>,
    /// Account and storage key preimages required during execution.
    pub keys: Vec<Bytes>,
    /// The lowest block number referenced by a BLOCKHASH opcode during execution.
    pub lowest_block_number: Option<u64>,
}

impl ExecutionWitnessRecord {
    /// Records the state after execution using the given witness generation mode.
    pub fn record_executed_state<DB>(&mut self, statedb: &State<DB>, mode: ExecutionWitnessMode) {
        self.codes = match mode {
            ExecutionWitnessMode::Legacy => statedb
                .cache
                .contracts
                .values()
                .map(|code| code.original_bytes())
                .chain(statedb.bundle_state.contracts.values().map(|code| code.original_bytes()))
                .collect(),
            ExecutionWitnessMode::Canonical => {
                let mut codes = statedb
                    .cache
                    .contracts
                    .values()
                    .map(|code| code.original_bytes())
                    .filter(|code| !code.is_empty())
                    .collect::<Vec<_>>();
                codes.sort_unstable();
                codes
            }
        };

        for (address, account) in &statedb.cache.accounts {
            let hashed_address = keccak256(address);
            self.hashed_state.accounts.insert(
                hashed_address,
                account.account.as_ref().map(|account| (&account.info).into()),
            );

            let storage = self
                .hashed_state
                .storages
                .entry(hashed_address)
                .or_insert_with(|| HashedStorage::new(account.status.was_destroyed()));

            if let Some(account) = &account.account {
                self.keys.push(address.to_vec().into());

                for (slot, value) in &account.storage {
                    let slot = B256::from(*slot);
                    let hashed_slot = keccak256(slot);
                    storage.storage.insert(hashed_slot, *value);

                    self.keys.push(slot.into());
                }
            }
        }
        self.lowest_block_number =
            statedb.block_hashes.lowest().map(|(block_number, _)| block_number)
    }

    /// Creates the record from the state after execution.
    pub fn from_executed_state<DB>(state: &State<DB>, mode: ExecutionWitnessMode) -> Self {
        let mut record = Self::default();
        record.record_executed_state(state, mode);
        record
    }

    /// Converts this record into a complete execution witness by generating state proofs and
    /// fetching ancestor block headers.
    pub fn into_execution_witness<SP, HP>(
        self,
        state_provider: &SP,
        headers_provider: &HP,
        block_number: u64,
        mode: ExecutionWitnessMode,
    ) -> reth_storage_errors::provider::ProviderResult<alloy_rpc_types_debug::ExecutionWitness>
    where
        SP: reth_storage_api::StateProofProvider + ?Sized,
        HP: reth_storage_api::HeaderProvider + ?Sized,
        HP::Header: alloy_rlp::Encodable,
    {
        let Self { hashed_state, codes, keys, lowest_block_number } = self;

        let state = state_provider.witness(Default::default(), hashed_state, mode)?;
        let mut exec_witness =
            alloy_rpc_types_debug::ExecutionWitness { state, codes, keys, ..Default::default() };

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
}

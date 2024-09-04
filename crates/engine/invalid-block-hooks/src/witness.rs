use std::{collections::HashMap, path::PathBuf};

use alloy_rpc_types_debug::ExecutionWitness;
use reth_engine_primitives::InvalidBlockHook;
use reth_primitives::{keccak256, Receipt, SealedBlockWithSenders, SealedHeader, B256};
use reth_provider::{BlockExecutionOutput, StateProviderFactory};
use reth_trie::{updates::TrieUpdates, HashedPostState};

/// Generates a witness for the given block and saves it to a file.
#[derive(Debug)]
pub struct Witness<P> {
    output_directory: PathBuf,
    provider: P,
}

impl<P> Witness<P> {
    /// Creates a new witness hook.
    pub const fn new(output_directory: PathBuf, provider: P) -> Self {
        Self { output_directory, provider }
    }
}

impl<P> InvalidBlockHook for Witness<P>
where
    P: StateProviderFactory + Clone + Send + Sync + 'static,
{
    fn on_invalid_block(
        &self,
        _block: &SealedBlockWithSenders,
        header: &SealedHeader,
        output: &BlockExecutionOutput<Receipt>,
        _trie_updates: Option<(&TrieUpdates, B256)>,
    ) -> eyre::Result<()> {
        // TODO(alexey): add accessed accounts and storage slots to the hashed state and state
        // preimages

        let mut state_preimages = HashMap::new();

        for (address, account) in &output.state.state {
            let hashed_address = keccak256(address);
            state_preimages.insert(hashed_address, alloy_rlp::encode(address).into());

            for slot in account.storage.keys() {
                let slot = B256::from(slot.to_be_bytes());
                let hashed_slot = keccak256(slot);
                state_preimages.insert(hashed_slot, alloy_rlp::encode(slot).into());
            }
        }

        let hashed_state = HashedPostState::from_bundle_state(&output.state.state);
        let witness = self.provider.latest()?.witness(HashedPostState::default(), hashed_state)?;

        let path = self.output_directory.join(format!("{}_{}.json", header.number, header.hash()));
        let response = ExecutionWitness { witness, state_preimages: Some(state_preimages) };

        std::fs::write(path, serde_json::to_string(&response)?)?;

        Ok(())
    }
}

//! Execution witness generation for the SSZ Engine API extension.

use crate::engine_ssz_containers::ExecutionWitnessV1;
use alloy_rpc_types_engine::ExecutionData;
use reth_ethereum_primitives::{EthPrimitives, TransactionSigned};
use reth_evm::{execute::Executor, ConfigureEvm};
use reth_primitives_traits::{AlloyBlockHeader, Block};
use reth_provider::{HeaderProvider, StateProviderFactory};
use reth_revm::{database::StateProviderDatabase, witness::ExecutionWitnessRecord};
use reth_tasks::Runtime;
use reth_trie_common::ExecutionWitnessMode;
use std::{future::Future, pin::Pin};

/// Re-executes validated payloads against their parent state for `/payloads/witness`.
#[derive(Clone, Debug)]
pub struct EngineSszWitnessGenerator<Provider, Evm> {
    provider: Provider,
    evm_config: Evm,
    task_spawner: Runtime,
}

impl<Provider, Evm> EngineSszWitnessGenerator<Provider, Evm> {
    /// Creates a new witness generator.
    pub const fn new(provider: Provider, evm_config: Evm, task_spawner: Runtime) -> Self {
        Self { provider, evm_config, task_spawner }
    }
}

impl<Provider, Evm> EngineSszWitness for EngineSszWitnessGenerator<Provider, Evm>
where
    Provider: HeaderProvider + StateProviderFactory + Clone + Send + Sync + 'static,
    Provider::Header: alloy_rlp::Encodable,
    Evm: ConfigureEvm<Primitives = EthPrimitives> + 'static,
{
    fn generate_witness(
        &self,
        payload: ExecutionData,
    ) -> Pin<Box<dyn Future<Output = Result<ExecutionWitnessV1, String>> + Send + '_>> {
        let provider = self.provider.clone();
        let evm_config = self.evm_config.clone();
        let task_spawner = self.task_spawner.clone();

        Box::pin(async move {
            task_spawner
                .spawn_blocking(move || {
                    // A VALID newPayload need not be canonical or visible through the provider.
                    let block = payload
                        .payload
                        .try_into_block_with_sidecar::<TransactionSigned>(&payload.sidecar)
                        .map_err(|err| err.to_string())?
                        .try_into_recovered()
                        .map_err(|err| err.to_string())?;

                    let block_number = block.header().number;
                    let parent_hash = block.header().parent_hash;
                    let state_provider =
                        provider.state_by_block_hash(parent_hash).map_err(|err| err.to_string())?;
                    let block_executor =
                        evm_config.executor(StateProviderDatabase::new(state_provider));
                    let mode = ExecutionWitnessMode::Canonical;
                    let mut witness = None;
                    let mut first_header = block_number.saturating_sub(1);
                    block_executor
                        .execute_with_state_closure(&block, |statedb: &reth_revm::State<_>| {
                            if let Some((number, _)) = statedb.block_hashes.lowest() {
                                first_header = number;
                            }
                            witness =
                                Some(ExecutionWitnessRecord::new(statedb).into_execution_witness(
                                    &statedb.database.0,
                                    &provider,
                                    block_number,
                                    mode,
                                ));
                        })
                        .map_err(|err| err.to_string())?;

                    let witness = witness
                        .expect("state closure is called after successful execution")
                        .map_err(|err| err.to_string())?;

                    // Submitted payloads can extend a side chain. Number-based header lookups
                    // in the debug witness helper may belong to a different canonical ancestor.
                    let mut headers = Vec::new();
                    let mut hash = parent_hash;
                    for _ in first_header..block_number {
                        let header = provider
                            .header(hash)
                            .map_err(|err| err.to_string())?
                            .ok_or_else(|| format!("ancestor {hash} not found for witness"))?;
                        hash = header.parent_hash();
                        headers.push(alloy_rlp::encode(&header));
                    }
                    headers.reverse();

                    Ok(ExecutionWitnessV1 {
                        state: witness.state.into_iter().map(|bytes| bytes.to_vec()).collect(),
                        codes: witness.codes.into_iter().map(|bytes| bytes.to_vec()).collect(),
                        headers,
                    })
                })
                .await
                .map_err(|err| format!("witness generation task failed: {err}"))?
        })
    }
}

/// Generates an execution witness for a valid payload.
pub trait EngineSszWitness: Send + Sync + 'static {
    /// Generates a REST-SSZ execution witness after the submitted payload has been validated.
    fn generate_witness(
        &self,
        payload: ExecutionData,
    ) -> Pin<Box<dyn Future<Output = Result<ExecutionWitnessV1, String>> + Send + '_>>;
}

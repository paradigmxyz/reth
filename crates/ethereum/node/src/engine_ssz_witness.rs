//! Execution witness generation for the SSZ Engine API extension.

use crate::engine_ssz_containers::ExecutionWitnessV1;
use alloy_primitives::B256;
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
///
/// The parent state must be available through the provider (persisted, canonical in-memory,
/// or pending). Parents present only in the engine tree are reported as
/// [`EngineSszWitnessError::ParentStateUnavailable`]; the route then answers with the payload
/// status alone.
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
    ) -> Pin<
        Box<
            dyn Future<Output = Result<ExecutionWitnessV1, EngineSszWitnessError>> + Send + 'static,
        >,
    > {
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
                        .map_err(eyre::Report::new)?
                        .try_into_recovered()
                        .map_err(eyre::Report::new)?;

                    let block_number = block.header().number;
                    let parent_hash = block.header().parent_hash;
                    let state_provider =
                        provider.state_by_block_hash(parent_hash).map_err(|source| {
                            EngineSszWitnessError::ParentStateUnavailable {
                                parent: parent_hash,
                                source: eyre::Report::new(source),
                            }
                        })?;
                    let block_executor =
                        evm_config.executor(StateProviderDatabase::new(state_provider));
                    let mut witness = None;
                    let mut first_header = block_number.saturating_sub(1);
                    block_executor
                        .execute_with_state_closure(&block, |statedb: &reth_revm::State<_>| {
                            if let Some((number, _)) = statedb.block_hashes.lowest() {
                                first_header = number;
                            }
                            witness = Some(
                                ExecutionWitnessRecord::new(statedb)
                                    .into_execution_witness_without_headers(
                                        &statedb.database.0,
                                        ExecutionWitnessMode::Canonical,
                                    ),
                            );
                        })
                        .map_err(eyre::Report::new)?;

                    let witness = witness
                        .expect("state closure is called after successful execution")
                        .map_err(eyre::Report::new)?;

                    // Header numbers may refer to a different canonical ancestor.
                    let mut headers = Vec::new();
                    let mut hash = parent_hash;
                    for _ in first_header..block_number {
                        let header = provider
                            .header(hash)
                            .map_err(eyre::Report::new)?
                            .ok_or_else(|| eyre::eyre!("ancestor {hash} not found for witness"))?;
                        hash = header.parent_hash();
                        headers.push(alloy_rlp::encode(&header).into());
                    }
                    headers.reverse();

                    Ok(ExecutionWitnessV1 { state: witness.state, codes: witness.codes, headers })
                })
                .await
                .map_err(eyre::Report::new)?
        })
    }
}

/// Generates an execution witness for a valid payload.
pub trait EngineSszWitness: Send + Sync + 'static {
    /// Generates a REST-SSZ execution witness after the submitted payload has been validated.
    fn generate_witness(
        &self,
        payload: ExecutionData,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<ExecutionWitnessV1, EngineSszWitnessError>> + Send + 'static,
        >,
    >;
}

/// Failure to produce a witness for a validated payload.
#[derive(Debug)]
pub enum EngineSszWitnessError {
    /// The parent state is not available through the provider yet.
    ParentStateUnavailable {
        /// Parent block whose state is required.
        parent: B256,
        /// Provider failure while accessing the state.
        source: eyre::Report,
    },
    /// Witness execution or proof generation failed.
    Internal(eyre::Report),
}

impl From<eyre::Report> for EngineSszWitnessError {
    fn from(error: eyre::Report) -> Self {
        Self::Internal(error)
    }
}

impl std::fmt::Display for EngineSszWitnessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ParentStateUnavailable { parent, source } => {
                write!(f, "parent state {parent} is unavailable through the provider: {source}")
            }
            Self::Internal(error) => std::fmt::Display::fmt(error, f),
        }
    }
}

impl std::error::Error for EngineSszWitnessError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ParentStateUnavailable { source, .. } | Self::Internal(source) => {
                Some(source.as_ref())
            }
        }
    }
}

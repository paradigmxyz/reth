//! Execution witness generation and wire types for the SSZ Engine API extension.

use alloy_primitives::{Bytes, B256};
use alloy_rpc_types_engine::{
    ssz_engine_types::{Optional, PayloadStatus, PayloadStatusKind},
    ExecutionData,
};
use reth_ethereum_primitives::{EthPrimitives, TransactionSigned};
use reth_evm::{execute::Executor, ConfigureEvm};
use reth_primitives_traits::{AlloyBlockHeader, Block};
use reth_provider::{HeaderProvider, StateProvider, StateProviderBox, StateProviderFactory};
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
                    let block_executor = evm_config.executor(StateProviderDatabase::new(
                        state_provider.into_evm_state_provider(),
                    ));
                    let mut witness = None;
                    let mut first_header = block_number.saturating_sub(1);
                    block_executor
                        .execute_with_state_closure(&block, |statedb: &reth_revm::State<_>| {
                            if let Some((number, _)) = statedb.block_hashes.lowest() {
                                first_header = number;
                            }
                            witness = Some(
                                ExecutionWitnessRecord::new(statedb)
                                    .into_execution_witness_without_headers::<StateProviderBox>(
                                        &statedb.database,
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

/// A trie-node byte list in an [`ExecutionWitnessV1`].
pub type WitnessNodeV1 = Bytes;

/// A contract-code byte list in an [`ExecutionWitnessV1`].
pub type WitnessCodeV1 = Bytes;

/// An RLP-encoded header byte list in an [`ExecutionWitnessV1`].
pub type WitnessHeaderV1 = Bytes;

/// Canonical execution witness for `POST /payloads/witness`.
///
/// `state` and `codes` are produced in lexicographic ascending byte order. `headers` are
/// RLP-encoded and ordered by ascending block number; consecutive headers must be parent-linked.
/// These ordering rules are producer-side requirements from the execution-specs witness builder.
///
/// This is a REST-SSZ wire container, not the JSON-RPC debug witness shape.
#[derive(Clone, Debug, Default, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct ExecutionWitnessV1 {
    /// Hashed trie-node preimages required during execution and state-root recomputation.
    pub state: Vec<WitnessNodeV1>,
    /// Contract bytecode preimages required from the pre-state.
    pub codes: Vec<WitnessCodeV1>,
    /// RLP-encoded ancestor headers used for pre-state and `BLOCKHASH` correctness proofs.
    pub headers: Vec<WitnessHeaderV1>,
}

/// Canonical execution witness for `POST /payloads/witness`.
pub type ExecutionWitness = ExecutionWitnessV1;

/// REST-SSZ response for `POST /payloads/witness`.
///
/// The witness uses the Engine REST-SSZ `Optional[T]` encoding from execution-apis and is present
/// only when the payload status is `VALID`. A `VALID` status without a witness means the parent
/// state was not yet available through the provider (the parent is only known to the engine
/// tree); resubmitting the payload once forkchoice has made the parent canonical yields it.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode)]
pub struct PayloadStatusWithWitness {
    /// Result of processing the submitted payload.
    pub payload_status: PayloadStatus,
    /// Execution witness produced for a valid payload.
    pub witness: Optional<ExecutionWitnessV1>,
}

impl PayloadStatusWithWitness {
    /// Creates a response, converting the witness into the REST-SSZ `Optional[T]` representation.
    pub fn new(payload_status: PayloadStatus, witness: Option<ExecutionWitnessV1>) -> Self {
        let witness = match payload_status.status {
            PayloadStatusKind::Valid => witness.into(),
            _ => Optional::none(),
        };
        Self { payload_status, witness }
    }
}

/// Backwards-compatible alias for the experimental witness response name.
pub type NewPayloadWithWitnessResponseV1 = PayloadStatusWithWitness;

impl ssz::Decode for PayloadStatusWithWitness {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, ssz::DecodeError> {
        let mut builder = ssz::SszDecoderBuilder::new(bytes);
        builder.register_type::<PayloadStatus>()?;
        builder.register_type::<Optional<ExecutionWitnessV1>>()?;
        let mut decoder = builder.build()?;
        let response =
            Self { payload_status: decoder.decode_next()?, witness: decoder.decode_next()? };
        if response.witness.is_some() && response.payload_status.status != PayloadStatusKind::Valid
        {
            return Err(ssz::DecodeError::BytesInvalid(
                "execution witness is only valid for VALID payload status".into(),
            ))
        }
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssz::{Decode, Encode};

    fn assert_roundtrip<T>(value: &T)
    where
        T: Encode + Decode + PartialEq + core::fmt::Debug,
    {
        assert_eq!(T::from_ssz_bytes(&value.as_ssz_bytes()).unwrap(), *value);
    }

    #[test]
    fn witness_response_roundtrips_when_status_is_valid() {
        let payload_status = PayloadStatus {
            status: PayloadStatusKind::Valid,
            latest_valid_hash: Optional::none(),
            validation_error: Optional::none(),
        };
        let witness = ExecutionWitnessV1 {
            state: vec![vec![1, 2, 3].into()],
            codes: vec![vec![4, 5].into()],
            headers: vec![vec![6].into()],
        };
        let response = PayloadStatusWithWitness::new(payload_status, Some(witness));

        assert_roundtrip(&response);
    }

    #[test]
    fn witness_response_omits_witness_for_non_valid_status() {
        let payload_status = PayloadStatus {
            status: PayloadStatusKind::Syncing,
            latest_valid_hash: Optional::none(),
            validation_error: Optional::none(),
        };
        let response =
            PayloadStatusWithWitness::new(payload_status, Some(ExecutionWitnessV1::default()));

        assert!(response.witness.is_none());
        assert_roundtrip(&response);
    }
}

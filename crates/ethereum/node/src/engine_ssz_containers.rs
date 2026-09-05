//! REST-SSZ Engine API types and the experimental witness response.

use alloy_primitives::Bytes;
pub use alloy_rpc_types_engine::ssz_engine_types::*;

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
        let witness = match &payload_status.status {
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
        if response.witness.is_some() &&
            !matches!(response.payload_status.status, PayloadStatusKind::Valid)
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

    fn assert_roundtrip<T: Encode + Decode + PartialEq + std::fmt::Debug>(value: &T) {
        assert_eq!(&T::from_ssz_bytes(&value.as_ssz_bytes()).unwrap(), value);
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

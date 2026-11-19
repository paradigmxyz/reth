//! Experimental EIP-8288 profile using the pinned leanVM aggregation program.
//!
//! The header digest is Keccak-256 over canonical `message || public_key` pairs. The native
//! leanVM statement uses its own claim order; verification reconstructs those exact claims
//! from the block dependencies. This profile is not a finalized Ethereum consensus profile.

use alloy_consensus::Transaction;
use alloy_eips::eip8288::{LEANSPHINCS_SCHEME, MAX_LEANSIG_DEPS_PER_WRAPPER};
use alloy_primitives::{Bytes, Keccak256, B256};
use alloy_rlp::{Decodable, Encodable};
use lean_vm::{lean_sphincs, AggregateClaims, AggregateProof, OutputClaims};
use std::{
    collections::BTreeMap,
    sync::{Arc, Once, OnceLock},
};

pub use alloy_eips::eip8288::{Dependency, DependencyError, RecursiveStark};

/// Maximum canonical dependencies in an experimental recursive wrapper.
pub const MAX_AGGREGATE_DEPENDENCIES: usize = 16_384;
/// Maximum serialized aggregate proof accepted by this devnet profile.
pub const MAX_PROOF_BYTES: usize = 1_048_576;
/// Maximum encoded wrapper size in the experimental gossip protocol.
pub const MAX_WRAPPER_BYTES: usize = 2_097_152;
/// Pinned program and proof-system profile, separate from the unfinished protocol constants.
pub const PROFILE: &str = "eip8288-leanvm-84780fb42667bc288c0e567c125b7e10479fb568-keccak-v1";

/// A dependency or proof cannot establish the requested claim set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProofError {
    /// A frame declaration is malformed.
    #[error(transparent)]
    Dependency(#[from] DependencyError),
    /// A list is unsorted, duplicated, unsupported, or too large.
    #[error("invalid EIP-8288 dependency list")]
    Dependencies,
    /// Witness count does not match the direct dependency count.
    #[error("EIP-8288 witness count does not match dependencies")]
    WitnessCount,
    /// The witness does not verify its claimed key and message.
    #[error("invalid leanSPHINCS dependency signature")]
    Signature,
    /// The serialized proof is malformed or does not establish the expected claims.
    #[error("invalid EIP-8288 aggregate proof")]
    Proof,
    /// A declared hash differs from the dependencies reconstructed from transactions.
    #[error("EIP-8288 dependency coverage mismatch")]
    Coverage,
    /// A required dependency has no accompanying evidence.
    #[error("missing EIP-8288 dependency evidence")]
    MissingEvidence,
    /// Wrapper bytes are malformed, oversized, or contain an unsupported mode.
    #[error("invalid EIP-8288 wrapper encoding")]
    Wrapper,
    /// The prover could not produce the requested aggregate.
    #[error("EIP-8288 aggregation failed: {0}")]
    Aggregation(#[from] lean_vm::AggregateError),
}

/// Immutable wrapper evidence. Validation is shared across all transactions in that wrapper.
#[derive(Debug)]
pub struct DependencyEvidence {
    dependencies: Vec<Dependency>,
    witness: Witness,
    verified: OnceLock<Result<(), ProofError>>,
}

#[derive(Debug, PartialEq, Eq)]
enum Witness {
    Direct(Vec<Bytes>),
    Aggregate(Bytes),
}

/// A wire package of typed transaction bytes (or 32-byte hashes) and shared dependency evidence.
#[derive(Clone, Debug)]
pub struct DependencyWrapper {
    /// EIP-2718 transaction bytes, or a 32-byte reference to a previously announced transaction.
    pub transactions: Vec<Bytes>,
    /// Canonical dependency list and direct witnesses or aggregate proof.
    pub evidence: Arc<DependencyEvidence>,
}

impl DependencyWrapper {
    /// Decodes a bounded RLP `[transactions, mode, [dependencies, witnesses_or_proof]]` package.
    /// Transaction resolution and exact dependency coverage must be checked before admission.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProofError> {
        if bytes.len() > MAX_WRAPPER_BYTES {
            return Err(ProofError::Wrapper);
        }
        let mut bytes = bytes;
        let header = alloy_rlp::Header::decode(&mut bytes).map_err(|_| ProofError::Wrapper)?;
        if !header.list || header.payload_length != bytes.len() {
            return Err(ProofError::Wrapper);
        }
        let transactions = decode_list::<Bytes>(&mut bytes, MAX_AGGREGATE_DEPENDENCIES)?;
        if transactions.iter().any(|transaction| transaction.len() < 32) {
            return Err(ProofError::Wrapper);
        }
        let mode = u8::decode(&mut bytes).map_err(|_| ProofError::Wrapper)?;
        let content = alloy_rlp::Header::decode(&mut bytes).map_err(|_| ProofError::Wrapper)?;
        if !content.list || content.payload_length != bytes.len() {
            return Err(ProofError::Wrapper);
        }
        let dependencies = decode_list::<Dependency>(&mut bytes, MAX_AGGREGATE_DEPENDENCIES)?;
        let evidence = match mode {
            0 => DependencyEvidence::direct(
                dependencies,
                decode_list(&mut bytes, MAX_LEANSIG_DEPS_PER_WRAPPER)?,
            )?,
            1 => DependencyEvidence::aggregate(
                dependencies,
                Bytes::decode(&mut bytes).map_err(|_| ProofError::Wrapper)?,
            )?,
            _ => return Err(ProofError::Wrapper),
        };
        if !bytes.is_empty() {
            return Err(ProofError::Wrapper);
        }
        Ok(Self { transactions, evidence: Arc::new(evidence) })
    }

    /// Encodes the package without changing any transaction's canonical bytes or hash.
    pub fn encode(&self) -> Result<Bytes, ProofError> {
        if self.transactions.len() > MAX_AGGREGATE_DEPENDENCIES ||
            self.transactions.iter().any(|transaction| transaction.len() < 32)
        {
            return Err(ProofError::Wrapper);
        }
        let (mode, witnesses) = self.evidence.wire_witnesses();
        let direct_length: usize = witnesses.iter().map(Encodable::length).sum();
        let direct_header = alloy_rlp::Header { list: true, payload_length: direct_length };
        let witness_len =
            if mode == 0 { direct_header.length_with_payload() } else { witnesses[0].length() };
        let content = alloy_rlp::Header {
            list: true,
            payload_length: self.evidence.dependencies.length() + witness_len,
        };
        let outer = alloy_rlp::Header {
            list: true,
            payload_length: self.transactions.length() +
                mode.length() +
                content.length_with_payload(),
        };
        if outer.length_with_payload() > MAX_WRAPPER_BYTES {
            return Err(ProofError::Wrapper);
        }
        let mut bytes = Vec::with_capacity(outer.length_with_payload());
        outer.encode(&mut bytes);
        self.transactions.encode(&mut bytes);
        mode.encode(&mut bytes);
        content.encode(&mut bytes);
        self.evidence.dependencies.encode(&mut bytes);
        if mode == 0 {
            direct_header.encode(&mut bytes);
            for witness in witnesses {
                witness.encode(&mut bytes);
            }
        } else {
            witnesses[0].encode(&mut bytes);
        }
        Ok(bytes.into())
    }
}

fn decode_list<T: Decodable>(bytes: &mut &[u8], limit: usize) -> Result<Vec<T>, ProofError> {
    let header = alloy_rlp::Header::decode(bytes).map_err(|_| ProofError::Wrapper)?;
    if !header.list || header.payload_length > bytes.len() {
        return Err(ProofError::Wrapper);
    }
    let (mut payload, rest) = bytes.split_at(header.payload_length);
    *bytes = rest;
    let mut values = Vec::new();
    while !payload.is_empty() {
        if values.len() == limit {
            return Err(ProofError::Wrapper);
        }
        values.push(T::decode(&mut payload).map_err(|_| ProofError::Wrapper)?);
    }
    Ok(values)
}

impl PartialEq for DependencyEvidence {
    fn eq(&self, other: &Self) -> bool {
        self.dependencies == other.dependencies && self.witness == other.witness
    }
}

impl Eq for DependencyEvidence {}

impl DependencyEvidence {
    /// Constructs bounded direct evidence. Signature validity is checked by [`Self::verify`].
    pub fn direct(
        dependencies: Vec<Dependency>,
        signatures: Vec<Bytes>,
    ) -> Result<Self, ProofError> {
        validate_dependencies(&dependencies)?;
        if dependencies.len() > MAX_LEANSIG_DEPS_PER_WRAPPER ||
            dependencies.len() != signatures.len()
        {
            return Err(ProofError::WitnessCount);
        }
        if signatures.iter().any(|signature| signature.len() != lean_sphincs::SIG_SIZE) {
            return Err(ProofError::Signature);
        }
        Ok(Self { dependencies, witness: Witness::Direct(signatures), verified: OnceLock::new() })
    }

    /// Constructs bounded recursive evidence. Proof validity is checked by [`Self::verify`].
    pub fn aggregate(dependencies: Vec<Dependency>, proof: Bytes) -> Result<Self, ProofError> {
        validate_dependencies(&dependencies)?;
        if proof.len() > MAX_PROOF_BYTES || dependencies.is_empty() != proof.is_empty() {
            return Err(ProofError::Proof);
        }
        Ok(Self { dependencies, witness: Witness::Aggregate(proof), verified: OnceLock::new() })
    }

    /// Returns the canonical, resolved dependency list carried by this evidence.
    pub fn dependencies(&self) -> &[Dependency] {
        &self.dependencies
    }

    /// Verifies immutable witnesses once, including when shared by several pool transactions.
    pub fn verify(&self) -> Result<(), ProofError> {
        *self.verified.get_or_init(|| match &self.witness {
            Witness::Direct(signatures) => {
                for (dependency, signature) in self.dependencies.iter().zip(signatures) {
                    let signature =
                        signature.as_ref().try_into().map_err(|_| ProofError::Signature)?;
                    if !lean_sphincs::verify(
                        &lean_sphincs::LeanSphincsPublicKey::from_bytes(
                            &dependency.verification_key_hash.0,
                        ),
                        &dependency.data_hash.0,
                        &lean_sphincs::LeanSphincsSignature::from_bytes(signature),
                    ) {
                        return Err(ProofError::Signature);
                    }
                }
                Ok(())
            }
            Witness::Aggregate(proof) => verify_proof(&self.dependencies, proof),
        })
    }

    /// Checks that this verified wrapper covers every dependency of one transaction.
    pub fn covers(&self, dependencies: &[Dependency]) -> Result<(), ProofError> {
        if dependencies
            .iter()
            .any(|dependency| self.dependencies.binary_search(dependency).is_err())
        {
            return Err(ProofError::Coverage);
        }
        self.verify()
    }

    /// Returns the immutable evidence's retained byte size for conservative pool accounting.
    pub fn size(&self) -> usize {
        std::mem::size_of::<Self>() +
            self.dependencies.len() * std::mem::size_of::<Dependency>() +
            match &self.witness {
                Witness::Direct(signatures) => {
                    signatures.iter().map(|signature| signature.len()).sum()
                }
                Witness::Aggregate(proof) => proof.len(),
            }
    }

    /// Returns direct witnesses or the single recursive proof for wrapper serialization.
    pub fn wire_witnesses(&self) -> (u8, &[Bytes]) {
        match &self.witness {
            Witness::Direct(signatures) => (0, signatures),
            Witness::Aggregate(proof) => (1, std::slice::from_ref(proof)),
        }
    }
}

/// Computes the devnet header digest without allocating a concatenation buffer.
pub fn dependency_digest(dependencies: &[Dependency]) -> B256 {
    let mut hash = Keccak256::new();
    for dependency in dependencies {
        hash.update(dependency.data_hash);
        hash.update(dependency.verification_key_hash);
    }
    hash.finalize()
}

/// Reconstructs a block or wrapper's sorted, distinct dependencies from its transactions.
pub fn transaction_dependencies<'a, T: Transaction + 'a>(
    transactions: impl IntoIterator<Item = &'a T>,
) -> Result<Vec<Dependency>, ProofError> {
    let mut dependencies = Vec::new();
    for transaction in transactions {
        if let Some(frame) = transaction.frame_transaction() {
            dependencies.extend(frame.dependencies()?);
        }
    }
    dependencies.sort_unstable();
    dependencies.dedup();
    validate_dependencies(&dependencies)?;
    Ok(dependencies)
}

/// Checks the header digest and proof against the exact dependencies of the block.
pub fn verify_header(
    dependencies: &[Dependency],
    entry: &RecursiveStark,
) -> Result<(), ProofError> {
    if entry.block_deps_hash != dependency_digest(dependencies) {
        return Err(ProofError::Coverage);
    }
    verify_proof(dependencies, &entry.stark_proof)
}

/// Recursively combines verified evidence, retaining exactly the dependencies selected for output.
/// Call from a blocking proving worker, not an asynchronous executor thread.
pub fn prove(
    dependencies: &[Dependency],
    inputs: &[Arc<DependencyEvidence>],
) -> Result<RecursiveStark, ProofError> {
    validate_dependencies(dependencies)?;
    let block_deps_hash = dependency_digest(dependencies);
    if dependencies.is_empty() {
        return Ok(RecursiveStark { stark_proof: Bytes::new(), block_deps_hash });
    }
    for input in inputs {
        if input.dependencies == dependencies &&
            let Witness::Aggregate(proof) = &input.witness
        {
            input.verify()?;
            return Ok(RecursiveStark { stark_proof: proof.clone(), block_deps_hash });
        }
    }
    setup();
    let mut raw = BTreeMap::new();
    let mut children: Vec<(AggregateClaims, AggregateProof)> = Vec::new();
    let retained = claims(dependencies);
    for input in inputs {
        input.verify()?;
        match &input.witness {
            Witness::Direct(signatures) => {
                for (dependency, signature) in input.dependencies.iter().zip(signatures) {
                    if dependencies.binary_search(dependency).is_ok() &&
                        let std::collections::btree_map::Entry::Vacant(entry) =
                            raw.entry(*dependency)
                    {
                        let signature =
                            signature.as_ref().try_into().map_err(|_| ProofError::Signature)?;
                        entry.insert((
                            lean_sphincs::LeanSphincsPublicKey::from_bytes(
                                &dependency.verification_key_hash.0,
                            ),
                            dependency.data_hash.0,
                            lean_sphincs::LeanSphincsSignature::from_bytes(signature),
                        ));
                    }
                }
            }
            Witness::Aggregate(proof) if !input.dependencies.is_empty() => {
                if children.len() == lean_vm::MAX_RECURSIONS {
                    // Prune before recursively consolidating so unrelated pool claims do not
                    // accumulate across batches of child proofs.
                    let intermediate = AggregateClaims {
                        lean_sphincs: retained
                            .lean_sphincs
                            .iter()
                            .filter(|claim| {
                                children.iter().any(|(claims, _)| {
                                    claims.lean_sphincs.binary_search(claim).is_ok()
                                })
                            })
                            .copied()
                            .collect(),
                        ..Default::default()
                    };
                    let aggregate = lean_vm::aggregate(
                        &children,
                        vec![],
                        vec![],
                        None,
                        OutputClaims::Only(&intermediate),
                        1,
                    )?;
                    children.clear();
                    children.push(aggregate);
                }
                children.push((
                    claims(&input.dependencies),
                    AggregateProof::from_bytes(proof).ok_or(ProofError::Proof)?,
                ));
            }
            Witness::Aggregate(_) => {}
        }
    }
    // Claims already established by children do not need another raw signature verification.
    raw.retain(|dependency, _| {
        !inputs.iter().any(|input| {
            matches!(input.witness, Witness::Aggregate(_)) &&
                input.dependencies.binary_search(dependency).is_ok()
        })
    });
    let (_, proof) = lean_vm::aggregate(
        &children,
        vec![],
        raw.into_values().collect(),
        None,
        OutputClaims::Only(&retained),
        1,
    )?;
    let stark_proof = Bytes::from(proof.to_bytes());
    if stark_proof.len() > MAX_PROOF_BYTES {
        return Err(ProofError::Proof);
    }
    Ok(RecursiveStark { stark_proof, block_deps_hash })
}

fn setup() {
    static SETUP: Once = Once::new();
    SETUP.call_once(|| lean_vm::setup(0));
}

fn validate_dependencies(dependencies: &[Dependency]) -> Result<(), ProofError> {
    if dependencies.len() > MAX_AGGREGATE_DEPENDENCIES ||
        dependencies.iter().any(|dependency| dependency.scheme != LEANSPHINCS_SCHEME) ||
        dependencies.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(ProofError::Dependencies);
    }
    Ok(())
}

fn claims(dependencies: &[Dependency]) -> AggregateClaims {
    let mut lean_sphincs = dependencies
        .iter()
        .map(|dependency| {
            (
                lean_sphincs::LeanSphincsPublicKey::from_bytes(&dependency.verification_key_hash.0),
                dependency.data_hash.0,
            )
        })
        .collect::<Vec<_>>();
    lean_sphincs.sort_unstable();
    AggregateClaims { lean_sphincs, ..Default::default() }
}

fn verify_proof(dependencies: &[Dependency], proof: &[u8]) -> Result<(), ProofError> {
    validate_dependencies(dependencies)?;
    if proof.len() > MAX_PROOF_BYTES {
        return Err(ProofError::Proof);
    }
    if dependencies.is_empty() {
        return if proof.is_empty() { Ok(()) } else { Err(ProofError::Proof) };
    }
    setup();
    if !AggregateProof::from_bytes(proof).is_some_and(|proof| proof.verify(&claims(dependencies))) {
        return Err(ProofError::Proof);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_malformed_proofs_fail_closed() {
        let empty = prove(&[], &[]).unwrap();
        assert_eq!(verify_header(&[], &empty), Ok(()));
        assert_eq!(dependency_digest(&[]), alloy_primitives::keccak256([]));
        let dependency = Dependency {
            scheme: LEANSPHINCS_SCHEME,
            data_hash: B256::repeat_byte(1),
            verification_key_hash: B256::repeat_byte(2),
        };
        assert_eq!(verify_header(&[dependency], &empty), Err(ProofError::Coverage));
        assert_eq!(
            DependencyEvidence::direct(vec![dependency], vec![]).unwrap_err(),
            ProofError::WitnessCount
        );
        assert_eq!(
            DependencyEvidence::direct(vec![dependency, dependency], vec![]).unwrap_err(),
            ProofError::Dependencies
        );
        assert_eq!(
            DependencyEvidence::aggregate(vec![dependency], Bytes::new()).unwrap_err(),
            ProofError::Proof
        );
    }

    #[test]
    fn wrapper_encoding_is_bounded_and_canonical() {
        let wrapper = DependencyWrapper {
            transactions: vec![Bytes::from(vec![1; 32])],
            evidence: Arc::new(DependencyEvidence::aggregate(vec![], Bytes::new()).unwrap()),
        };
        let encoded = wrapper.encode().unwrap();
        let decoded = DependencyWrapper::decode(&encoded).unwrap();
        assert_eq!(decoded.transactions, wrapper.transactions);
        assert_eq!(decoded.evidence, wrapper.evidence);
        assert_eq!(decoded.encode().unwrap(), encoded);
        let mut trailing = encoded.to_vec();
        trailing.push(0);
        assert_eq!(DependencyWrapper::decode(&trailing).unwrap_err(), ProofError::Wrapper);
        assert_eq!(
            DependencyWrapper::decode(&vec![0; MAX_WRAPPER_BYTES + 1]).unwrap_err(),
            ProofError::Wrapper
        );
    }

    #[test]
    fn real_signatures_recursion_and_discard_preserve_exact_coverage() {
        let seed = [7; lean_sphincs::SECRET_KEY_LEN];
        let key = lean_sphincs::key_gen(&seed);
        let mut dependencies = Vec::new();
        let mut signatures = Vec::new();
        for value in [1, 2] {
            let message = [value; 32];
            dependencies.push(Dependency {
                scheme: LEANSPHINCS_SCHEME,
                data_hash: B256::from(message),
                verification_key_hash: B256::from(key.to_bytes()),
            });
            signatures
                .push(Bytes::copy_from_slice(&lean_sphincs::sign(&seed, &message).to_bytes()));
        }
        let direct =
            Arc::new(DependencyEvidence::direct(dependencies.clone(), signatures.clone()).unwrap());
        assert_eq!(direct.verify(), Ok(()));
        let leaf = prove(&dependencies, &[Arc::clone(&direct)]).unwrap();
        assert_eq!(verify_header(&dependencies, &leaf), Ok(()));
        let child = Arc::new(
            DependencyEvidence::aggregate(dependencies.clone(), leaf.stark_proof).unwrap(),
        );
        let reused = prove(&dependencies, &[Arc::clone(&child)]).unwrap();
        assert_eq!(reused.stark_proof.as_ref(), child.wire_witnesses().1[0].as_ref());
        let retained = &dependencies[..1];
        let root = prove(retained, &[child, direct]).unwrap();
        assert_eq!(verify_header(retained, &root), Ok(()));
        assert_eq!(verify_header(&dependencies, &root), Err(ProofError::Coverage));
        let wrong = RecursiveStark {
            block_deps_hash: dependency_digest(&dependencies),
            stark_proof: root.stark_proof,
        };
        assert_eq!(verify_header(&dependencies, &wrong), Err(ProofError::Proof));
        signatures[0] = Bytes::from(vec![0; lean_sphincs::SIG_SIZE]);
        let forged = DependencyEvidence::direct(dependencies, signatures).unwrap();
        assert_eq!(forged.verify(), Err(ProofError::Signature));
    }
}

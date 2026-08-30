use sha2::{Digest, Sha256};

use crate::{
    path::{parse_path, ParseError},
    schema::{
        receipt_log_address_gindex, resolve_v0, validate_runtime_bounds, BoundsError, GindexError,
        SCHEMA_DIGEST, SCHEMA_ID,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidAddressLength {
    pub actual: usize,
}

pub fn address_target_node(value_ssz: &[u8]) -> Result<[u8; 32], InvalidAddressLength> {
    if value_ssz.len() != 20 {
        return Err(InvalidAddressLength { actual: value_ssz.len() });
    }

    let mut node = [0_u8; 32];
    node[..20].copy_from_slice(value_ssz);
    Ok(node)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofError {
    ZeroGindex,
    WrongBranchLength { expected: usize, actual: usize },
}

fn hash_pair(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(left);
    hasher.update(right);

    let digest = hasher.finalize();
    let mut node = [0_u8; 32];
    node.copy_from_slice(&digest);
    node
}

pub fn verify_branch(
    target_node: [u8; 32],
    mut gindex: u64,
    proof: &[[u8; 32]],
    expected_root: [u8; 32],
) -> Result<bool, ProofError> {
    if gindex == 0 {
        return Err(ProofError::ZeroGindex);
    }

    let expected_length = (u64::BITS - 1 - gindex.leading_zeros()) as usize;
    if proof.len() != expected_length {
        return Err(ProofError::WrongBranchLength {
            expected: expected_length,
            actual: proof.len(),
        });
    }

    let mut current = target_node;
    for sibling in proof {
        current = if gindex & 1 == 0 {
            hash_pair(&current, sibling)
        } else {
            hash_pair(sibling, &current)
        };
        gindex >>= 1;
    }

    Ok(gindex == 1 && current == expected_root)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvelopeError {
    WrongSchema,
    WrongSchemaDigest,
    InvalidPath(ParseError),
    UnsupportedPath,
    InvalidBounds(BoundsError),
    InvalidGindex(GindexError),
    InvalidValue(InvalidAddressLength),
    InvalidProof(ProofError),
}

pub fn verify_receipt_log_address(
    schema_id: &str,
    schema_digest: [u8; 32],
    path: &str,
    receipt_log_counts: &[usize],
    value_ssz: &[u8],
    proof: &[[u8; 32]],
    expected_root: [u8; 32],
) -> Result<bool, EnvelopeError> {
    if schema_id != SCHEMA_ID {
        return Err(EnvelopeError::WrongSchema);
    }
    if schema_digest != SCHEMA_DIGEST {
        return Err(EnvelopeError::WrongSchemaDigest);
    }

    let tokens = parse_path(path).map_err(EnvelopeError::InvalidPath)?;
    let resolved = resolve_v0(&tokens).map_err(|_| EnvelopeError::UnsupportedPath)?;
    validate_runtime_bounds(resolved, receipt_log_counts).map_err(EnvelopeError::InvalidBounds)?;
    let gindex = receipt_log_address_gindex(resolved).map_err(EnvelopeError::InvalidGindex)?;
    let target_node = address_target_node(value_ssz).map_err(EnvelopeError::InvalidValue)?;

    verify_branch(target_node, gindex, proof, expected_root).map_err(EnvelopeError::InvalidProof)
}

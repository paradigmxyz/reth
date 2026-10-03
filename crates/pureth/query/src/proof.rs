use alloy_primitives::B256;
use sha2::{Digest, Sha256};

use crate::{
    path::{parse_path, ParseError},
    schema::{
        compose_gindices, receipt_log_address_gindex, receipt_logs_gindex, resolve,
        validate_runtime_lengths, BoundsError, GindexError, SCHEMA_ID,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidAddressLength {
    pub actual: usize,
}

pub fn address_target_node(value_ssz: &[u8]) -> Result<B256, InvalidAddressLength> {
    if value_ssz.len() != 20 {
        return Err(InvalidAddressLength { actual: value_ssz.len() });
    }

    let mut node = [0_u8; 32];
    node[..20].copy_from_slice(value_ssz);
    Ok(B256::from(node))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofError {
    ZeroGindex,
    WrongBranchLength { expected: usize, actual: usize },
    RootMismatch,
    InvalidLengthNode { gindex: u64 },
}

fn hash_pair(left: &B256, right: &B256) -> B256 {
    let mut hasher = Sha256::new();
    hasher.update(left);
    hasher.update(right);

    B256::from(<[u8; 32]>::from(hasher.finalize()))
}

pub fn verify_branch(
    target_node: B256,
    mut gindex: u64,
    proof: &[B256],
    expected_root: B256,
) -> Result<(), ProofError> {
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

    if gindex != 1 || current != expected_root {
        return Err(ProofError::RootMismatch);
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvelopeError {
    WrongSchema,
    InvalidPath(ParseError),
    UnsupportedPath,
    InvalidBounds(BoundsError),
    InvalidGindex(GindexError),
    InvalidValue(InvalidAddressLength),
    InvalidProof(ProofError),
}

pub fn verify_receipt_log_address(
    schema_id: &str,
    path: &str,
    value_ssz: &[u8],
    proof: &[B256],
    expected_root: B256,
) -> Result<(), EnvelopeError> {
    if schema_id != SCHEMA_ID {
        return Err(EnvelopeError::WrongSchema);
    }
    let tokens = parse_path(path).map_err(EnvelopeError::InvalidPath)?;
    let resolved = resolve(&tokens).map_err(|_| EnvelopeError::UnsupportedPath)?;
    let gindex = receipt_log_address_gindex(resolved).map_err(EnvelopeError::InvalidGindex)?;
    let logs_gindex = receipt_logs_gindex(resolved).map_err(EnvelopeError::InvalidGindex)?;
    let logs_length_gindex =
        compose_gindices(logs_gindex, 3).map_err(EnvelopeError::InvalidGindex)?;
    let target_node = address_target_node(value_ssz).map_err(EnvelopeError::InvalidValue)?;
    verify_branch(target_node, gindex, proof, expected_root)
        .map_err(EnvelopeError::InvalidProof)?;

    let (receipt_count, log_count) = authenticated_lengths(gindex, logs_length_gindex, proof)
        .map_err(EnvelopeError::InvalidProof)?;
    validate_runtime_lengths(resolved, receipt_count, log_count)
        .map_err(EnvelopeError::InvalidBounds)?;

    Ok(())
}

fn authenticated_lengths(
    target_gindex: u64,
    logs_length_gindex: u64,
    proof: &[B256],
) -> Result<(usize, usize), ProofError> {
    let receipt_count = decode_length_node(
        proof_node(target_gindex, proof, 3).ok_or(ProofError::InvalidLengthNode { gindex: 3 })?,
        3,
    )?;
    let log_count = decode_length_node(
        proof_node(target_gindex, proof, logs_length_gindex)
            .ok_or(ProofError::InvalidLengthNode { gindex: logs_length_gindex })?,
        logs_length_gindex,
    )?;

    Ok((receipt_count, log_count))
}

fn proof_node(target_gindex: u64, proof: &[B256], wanted_gindex: u64) -> Option<&B256> {
    let mut current = target_gindex;
    proof.iter().find(|_| {
        let found = current ^ 1 == wanted_gindex;
        current >>= 1;
        found
    })
}

fn decode_length_node(node: &B256, gindex: u64) -> Result<usize, ProofError> {
    let bytes = node.as_slice();
    if bytes[8..].iter().any(|byte| *byte != 0) {
        return Err(ProofError::InvalidLengthNode { gindex });
    }

    let length = u64::from_le_bytes(bytes[..8].try_into().expect("length slice is eight bytes"));
    usize::try_from(length).map_err(|_| ProofError::InvalidLengthNode { gindex })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn length_node(length: u64) -> B256 {
        let mut node = [0; 32];
        node[..8].copy_from_slice(&length.to_le_bytes());
        B256::from(node)
    }

    fn branch_root(mut node: B256, mut gindex: u64, proof: &[B256]) -> B256 {
        for sibling in proof {
            node =
                if gindex & 1 == 0 { hash_pair(&node, sibling) } else { hash_pair(sibling, &node) };
            gindex >>= 1;
        }
        node
    }

    #[test]
    fn bounds_use_authenticated_length_nodes() {
        let target = address_target_node(&[0x11; 20]).unwrap();
        let mut branch = vec![B256::ZERO; 9];
        branch[3] = length_node(1);
        branch[8] = length_node(1);
        let root = branch_root(target, 576, &branch);

        assert_eq!(
            verify_receipt_log_address(
                SCHEMA_ID,
                "[0].logs[0].address",
                &[0x11; 20],
                &branch,
                root,
            ),
            Ok(())
        );

        branch[3] = length_node(0);
        let root = branch_root(target, 576, &branch);
        assert_eq!(
            verify_receipt_log_address(
                SCHEMA_ID,
                "[0].logs[0].address",
                &[0x11; 20],
                &branch,
                root,
            ),
            Err(EnvelopeError::InvalidBounds(BoundsError::LogOutOfBounds))
        );

        branch[3] = length_node(1);
        branch[8] = length_node(0);
        let root = branch_root(target, 576, &branch);
        assert_eq!(
            verify_receipt_log_address(
                SCHEMA_ID,
                "[0].logs[0].address",
                &[0x11; 20],
                &branch,
                root,
            ),
            Err(EnvelopeError::InvalidBounds(BoundsError::ReceiptOutOfBounds))
        );
    }

    #[test]
    fn rejects_noncanonical_length_nodes() {
        let target = address_target_node(&[0x11; 20]).unwrap();
        let mut branch = vec![B256::ZERO; 9];
        branch[3] = length_node(1);
        branch[3][8] = 1;
        branch[8] = length_node(1);
        let root = branch_root(target, 576, &branch);

        assert_eq!(
            verify_receipt_log_address(
                SCHEMA_ID,
                "[0].logs[0].address",
                &[0x11; 20],
                &branch,
                root,
            ),
            Err(EnvelopeError::InvalidProof(ProofError::InvalidLengthNode { gindex: 73 }))
        );
    }
}

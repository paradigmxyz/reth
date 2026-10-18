use alloy_primitives::B256;
use sha2::{Digest, Sha256};

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
}

fn hash_pair(left: &B256, right: &B256) -> B256 {
    let mut hasher = Sha256::new();
    hasher.update(left);
    hasher.update(right);

    B256::from(<[u8; 32]>::from(hasher.finalize()))
}

pub fn verify_branch(
    target_node: B256,
    mut gindex: u128,
    proof: &[B256],
    expected_root: B256,
) -> Result<(), ProofError> {
    if gindex == 0 {
        return Err(ProofError::ZeroGindex);
    }

    let expected_length = (u128::BITS - 1 - gindex.leading_zeros()) as usize;
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

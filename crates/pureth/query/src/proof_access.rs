use crate::{
    resolve_receipt_log_address, verify_branch, ProofError, ReceiptResolutionError, ResolvedPath,
};
use alloy_primitives::{Address, B256};
use reth_pureth_receipt::{ReceiptSnapshot, RetainedNode};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiptLogAddressProof {
    pub address: Address,
    pub gindex: u64,
    pub branch: Vec<B256>,
    pub root: B256,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProofAccessError {
    Resolution(ReceiptResolutionError),
    MissingChildren { gindex: u64 },
    TargetMismatch { gindex: u64 },
    InvalidProof(ProofError),
}

pub fn prove_receipt_log_address(
    snapshot: &ReceiptSnapshot,
    path: ResolvedPath,
) -> Result<ReceiptLogAddressProof, ProofAccessError> {
    let (address, gindex) = resolve_receipt_log_address(snapshot.receipts(), path)
        .map_err(ProofAccessError::Resolution)?;
    let (target, branch) = node_and_branch(snapshot.tree(), gindex)?;
    let expected_target = B256::right_padding_from(address.as_slice());

    if target != expected_target {
        return Err(ProofAccessError::TargetMismatch { gindex });
    }

    let root = snapshot.root();
    verify_branch(target, gindex, &branch, root).map_err(ProofAccessError::InvalidProof)?;

    Ok(ReceiptLogAddressProof { address, gindex, branch, root })
}

fn node_and_branch(
    root: &RetainedNode,
    gindex: u64,
) -> Result<(B256, Vec<B256>), ProofAccessError> {
    if gindex == 0 {
        return Err(ProofAccessError::InvalidProof(ProofError::ZeroGindex));
    }

    let depth = u64::BITS - 1 - gindex.leading_zeros();
    let mut current = root;
    let mut current_gindex = 1;
    let mut branch = Vec::with_capacity(depth as usize);

    for shift in (0..depth).rev() {
        let children = current
            .children()
            .ok_or(ProofAccessError::MissingChildren { gindex: current_gindex })?;
        let child_index = ((gindex >> shift) & 1) as usize;
        branch.push(children[child_index ^ 1].root());
        current = &children[child_index];
        current_gindex = current_gindex * 2 + child_index as u64;
    }

    branch.reverse();
    Ok((current.root(), branch))
}

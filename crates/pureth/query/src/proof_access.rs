use crate::ProofError;
use alloy_primitives::B256;
use reth_pureth_ssz::RetainedNode;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProofAccessError {
    MissingChildren { gindex: u128 },
    InvalidProof(ProofError),
}

pub(crate) fn node_and_branch(
    root: &RetainedNode,
    gindex: u128,
) -> Result<(B256, Vec<B256>), ProofAccessError> {
    if gindex == 0 {
        return Err(ProofAccessError::InvalidProof(ProofError::ZeroGindex));
    }

    let depth = u128::BITS - 1 - gindex.leading_zeros();
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
        current_gindex = gindex >> shift;
    }

    branch.reverse();
    Ok((current.root(), branch))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verify_branch;
    use alloy_primitives::b256;

    #[test]
    fn retained_access_and_verification_support_wide_paths() {
        for (gindex, expected_root) in [
            (
                (1_u128 << 64) | 1,
                b256!("567083eb829b83e1c488500af2ddbf3a403d437513f584c1bd45fcd6d9f0477e"),
            ),
            (
                1_u128 << 127,
                b256!("ead2852364d533d84a04f8722fe28bcc4867d06ac7d2643d46e383f07581a7a0"),
            ),
            (u128::MAX, b256!("df7a7d7c48cc4a30a81df99ccebc73c270cf16314a4587a584f4c5ec0788f659")),
        ] {
            let target = B256::repeat_byte(0x11);
            let depth = u128::BITS - 1 - gindex.leading_zeros();
            let mut tree = RetainedNode::leaf(target);
            for bit in 0..depth {
                tree = if (gindex >> bit) & 1 == 0 {
                    RetainedNode::pair(tree, RetainedNode::zero())
                } else {
                    RetainedNode::pair(RetainedNode::zero(), tree)
                };
            }

            assert_eq!(tree.root(), expected_root);
            let (actual_target, branch) = node_and_branch(&tree, gindex).unwrap();
            assert_eq!(actual_target, target);
            assert_eq!(branch, vec![B256::ZERO; depth as usize]);
            assert_eq!(verify_branch(target, gindex, &branch, expected_root), Ok(()));
            assert_eq!(
                verify_branch(B256::ZERO, gindex, &branch, expected_root),
                Err(ProofError::RootMismatch),
            );
            assert_eq!(
                verify_branch(target, gindex, &vec![B256::ZERO; 128], expected_root),
                Err(ProofError::WrongBranchLength { expected: depth as usize, actual: 128 }),
            );
        }
    }

    #[test]
    fn access_rejects_zero_and_reports_wide_missing_children() {
        let leaf = RetainedNode::zero();
        assert_eq!(
            node_and_branch(&leaf, 0),
            Err(ProofAccessError::InvalidProof(ProofError::ZeroGindex)),
        );
        let mut tree = RetainedNode::zero();
        for _ in 0..64 {
            tree = RetainedNode::pair(RetainedNode::zero(), tree);
        }
        assert_eq!(
            node_and_branch(&tree, (1_u128 << 66) - 1),
            Err(ProofAccessError::MissingChildren { gindex: (1_u128 << 65) - 1 }),
        );
    }
}

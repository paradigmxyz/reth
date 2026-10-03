use alloy_primitives::B256;
use std::{fmt, vec::IntoIter};

#[derive(Debug)]
pub struct RetainedNode {
    root: B256,
    children: Option<Box<[Self; 2]>>,
}

impl RetainedNode {
    pub(super) const fn leaf(root: B256) -> Self {
        Self { root, children: None }
    }

    pub(super) const fn zero() -> Self {
        Self::leaf(B256::ZERO)
    }

    pub(super) fn pair(left: Self, right: Self) -> Self {
        let mut bytes = [0_u8; 64];
        bytes[..32].copy_from_slice(left.root.as_slice());
        bytes[32..].copy_from_slice(right.root.as_slice());

        Self { root: tree_hash::merkle_root(&bytes, 2), children: Some(Box::new([left, right])) }
    }

    pub const fn root(&self) -> B256 {
        self.root
    }

    pub fn children(&self) -> Option<&[Self; 2]> {
        self.children.as_deref()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeConstructionError {
    InvalidWidth { width: usize },
    WidthExceeded { actual: usize, width: usize },
    ProgressiveCapacityOverflow,
}

impl fmt::Display for TreeConstructionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidWidth { width } => {
                write!(f, "Merkle width {width} must be a nonzero power of two")
            }
            Self::WidthExceeded { actual, width } => {
                write!(f, "{actual} nodes do not fit in Merkle width {width}")
            }
            Self::ProgressiveCapacityOverflow => {
                write!(f, "progressive group size overflowed usize")
            }
        }
    }
}

impl std::error::Error for TreeConstructionError {}

pub(super) fn merkleize_fixed(
    mut nodes: Vec<RetainedNode>,
    width: usize,
) -> Result<RetainedNode, TreeConstructionError> {
    if width == 0 || !width.is_power_of_two() {
        return Err(TreeConstructionError::InvalidWidth { width });
    }

    if nodes.len() > width {
        return Err(TreeConstructionError::WidthExceeded { actual: nodes.len(), width });
    }

    nodes.resize_with(width, RetainedNode::zero);

    while nodes.len() > 1 {
        let mut children = nodes.into_iter();
        let mut parents = Vec::with_capacity(children.len() / 2);

        while let Some(left) = children.next() {
            let right = children.next().expect("a complete level has an even node count");
            parents.push(RetainedNode::pair(left, right));
        }

        nodes = parents;
    }

    Ok(nodes.pop().expect("a nonzero width leaves one root"))
}

pub(super) fn merkleize_progressive(
    nodes: Vec<RetainedNode>,
) -> Result<RetainedNode, TreeConstructionError> {
    progressive_level(&mut nodes.into_iter(), 1)
}

fn progressive_level(
    nodes: &mut IntoIter<RetainedNode>,
    width: usize,
) -> Result<RetainedNode, TreeConstructionError> {
    if nodes.len() == 0 {
        return Ok(RetainedNode::zero());
    }

    let group = nodes.by_ref().take(width).collect();
    let left = merkleize_fixed(group, width)?;

    let right = if nodes.len() == 0 {
        RetainedNode::zero()
    } else {
        let next_width =
            width.checked_mul(4).ok_or(TreeConstructionError::ProgressiveCapacityOverflow)?;
        progressive_level(nodes, next_width)?
    };

    Ok(RetainedNode::pair(left, right))
}

pub(super) fn mix_in_length(contents: RetainedNode, length: usize) -> RetainedNode {
    let mut chunk = [0_u8; 32];
    let encoded = length.to_le_bytes();
    chunk[..encoded.len()].copy_from_slice(&encoded);

    RetainedNode::pair(contents, RetainedNode::leaf(B256::from(chunk)))
}

pub(super) fn progressive_byte_list(bytes: &[u8]) -> Result<RetainedNode, TreeConstructionError> {
    let nodes = bytes
        .chunks(32)
        .map(|bytes| {
            let mut chunk = [0_u8; 32];
            chunk[..bytes.len()].copy_from_slice(bytes);
            RetainedNode::leaf(B256::from(chunk))
        })
        .collect();

    Ok(mix_in_length(merkleize_progressive(nodes)?, bytes.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::b256;

    #[test]
    fn pair_retains_children_and_hashes_in_order() {
        let left = B256::repeat_byte(0x11);
        let right = B256::repeat_byte(0x22);
        let node = RetainedNode::pair(RetainedNode::leaf(left), RetainedNode::leaf(right));

        let mut bytes = [0_u8; 64];
        bytes[..32].copy_from_slice(left.as_slice());
        bytes[32..].copy_from_slice(right.as_slice());

        let children = node.children().unwrap();
        assert_eq!(children[0].root(), left);
        assert_eq!(children[1].root(), right);
        assert_eq!(node.root(), tree_hash::merkle_root(&bytes, 2));
        assert!(children[0].children().is_none());
    }

    #[test]
    fn fixed_tree_preserves_nodes_and_padding() {
        let value = B256::repeat_byte(0x33);
        let node = merkleize_fixed(vec![RetainedNode::leaf(value)], 4).unwrap();

        assert_eq!(node.root(), tree_hash::merkle_root(value.as_slice(), 4));

        let left = &node.children().unwrap()[0];
        let leaves = left.children().unwrap();
        assert_eq!(leaves[0].root(), value);
        assert_eq!(leaves[1].root(), B256::ZERO);
    }

    #[test]
    fn fixed_tree_rejects_invalid_shapes() {
        for width in [0, 3] {
            assert_eq!(
                merkleize_fixed(Vec::new(), width).unwrap_err(),
                TreeConstructionError::InvalidWidth { width },
            );
        }

        let nodes = (0..5).map(|_| RetainedNode::zero()).collect();
        assert_eq!(
            merkleize_fixed(nodes, 4).unwrap_err(),
            TreeConstructionError::WidthExceeded { actual: 5, width: 4 },
        );
    }

    #[test]
    fn progressive_boundary_roots_match_reference() {
        for (count, expected) in [
            (5_usize, b256!("183886e81b2e887d5960b2fa49b3464eabee62ec55ff5e6ee6f7e0495d8a01d1")),
            (6_usize, b256!("690beb7f075e2dc91699aa3ee9354687772923889ce755458cb405ae95e34055")),
            (20, b256!("84c6ac351ba4a6bcb85ea189e3b5959fc9824dd28c3c3384a1f8fad179725e18")),
            (21, b256!("93589633f10a1e8fe51bef0481731c7c19d7a87269127b6c6a19720668ee47da")),
            (22, b256!("e79bdcda4e58dd09c4b855964e1f1c01c99e215b6e01602f5302763effaf8637")),
        ] {
            let nodes = (1..=count)
                .map(|value| RetainedNode::leaf(B256::repeat_byte(u8::try_from(value).unwrap())))
                .collect();

            let tree = mix_in_length(merkleize_progressive(nodes).unwrap(), count);
            assert_eq!(tree.root(), expected);
        }
    }

    #[test]
    fn progressive_bytes_preserve_logical_length() {
        assert_eq!(merkleize_progressive(Vec::new()).unwrap().root(), B256::ZERO,);
        assert_ne!(
            progressive_byte_list(&[1]).unwrap().root(),
            progressive_byte_list(&[1, 0]).unwrap().root(),
        );
    }

    #[test]
    fn byte_chunks_are_padded_and_length_is_in_bytes() {
        for length in [0_usize, 1, 31, 32, 33, 160, 161] {
            let tree = progressive_byte_list(&vec![0xab; length]).unwrap();
            let children = tree.children().unwrap();
            let mut chunk = [0_u8; 32];
            chunk[..8].copy_from_slice(&u64::try_from(length).unwrap().to_le_bytes());
            assert_eq!(children[1].root(), B256::from(chunk));
            if length == 0 {
                assert_eq!(children[0].root(), B256::ZERO);
                assert!(children[0].children().is_none());
            } else {
                chunk.fill(0);
                chunk[..length.min(32)].fill(0xab);
                assert_eq!(children[0].children().unwrap()[0].root(), B256::from(chunk));
            }
        }
        let one = progressive_byte_list(&[1]).unwrap();
        let trailing_zero = progressive_byte_list(&[1, 0]).unwrap();
        assert_eq!(one.children().unwrap()[0].root(), trailing_zero.children().unwrap()[0].root());
        assert_ne!(one.root(), trailing_zero.root());
    }
}

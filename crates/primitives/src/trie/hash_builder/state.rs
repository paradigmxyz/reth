use super::{super::TrieMask, HashBuilderValue};
use bytes::Buf;
use reth_codecs::{derive_arbitrary, Compact};
use serde::{Deserialize, Serialize};

/// The hash builder state for storing in the database.
/// Check the `reth-trie` crate for more info on hash builder.
#[derive_arbitrary(compact)]
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct HashBuilderState {
    /// The current key.
    pub key: Vec<u8>,
    /// The builder stack.
    pub stack: Vec<Vec<u8>>,
    /// The current node value.
    pub value: HashBuilderValue,

    /// Group masks.
    pub groups: Vec<TrieMask>,
    /// Tree masks.
    pub tree_masks: Vec<TrieMask>,
    /// Hash masks.
    pub hash_masks: Vec<TrieMask>,

    /// Flag indicating if the current node is stored in the database.
    pub stored_in_database: bool,
}

impl Compact for HashBuilderState {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let mut len = 0;

        len += self.key.to_compact(buf);

        buf.put_u16(self.stack.len() as u16);
        len += 2;
        for item in self.stack.iter() {
            buf.put_u16(item.len() as u16);
            buf.put_slice(&item[..]);
            len += 2 + item.len();
        }

        len += self.value.to_compact(buf);

        buf.put_u16(self.groups.len() as u16);
        len += 2;
        for item in self.groups.iter() {
            len += item.to_compact(buf);
        }

        buf.put_u16(self.tree_masks.len() as u16);
        len += 2;
        for item in self.tree_masks.iter() {
            len += item.to_compact(buf);
        }

        buf.put_u16(self.hash_masks.len() as u16);
        len += 2;
        for item in self.hash_masks.iter() {
            len += item.to_compact(buf);
        }

        buf.put_u8(self.stored_in_database as u8);
        len += 1;
        len
    }

    fn from_compact(buf: &[u8], _len: usize) -> (Self, &[u8]) {
        let (key, mut buf) = Vec::from_compact(buf, 0);

        let stack_len = buf.get_u16() as usize;
        let mut stack = Vec::with_capacity(stack_len);
        for _ in 0..stack_len {
            let item_len = buf.get_u16() as usize;
            stack.push(Vec::from(&buf[..item_len]));
            buf.advance(item_len);
        }

        let (value, mut buf) = HashBuilderValue::from_compact(buf, 0);

        let groups_len = buf.get_u16() as usize;
        let mut groups = Vec::with_capacity(groups_len);
        for _ in 0..groups_len {
            let (item, rest) = TrieMask::from_compact(buf, 0);
            groups.push(item);
            buf = rest;
        }

        let tree_masks_len = buf.get_u16() as usize;
        let mut tree_masks = Vec::with_capacity(tree_masks_len);
        for _ in 0..tree_masks_len {
            let (item, rest) = TrieMask::from_compact(buf, 0);
            tree_masks.push(item);
            buf = rest;
        }

        let hash_masks_len = buf.get_u16() as usize;
        let mut hash_masks = Vec::with_capacity(hash_masks_len);
        for _ in 0..hash_masks_len {
            let (item, rest) = TrieMask::from_compact(buf, 0);
            hash_masks.push(item);
            buf = rest;
        }

        let stored_in_database = buf.get_u8() != 0;
        (Self { key, stack, value, groups, tree_masks, hash_masks, stored_in_database }, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn hash_builder_state_regression() {
        let mut state = HashBuilderState::default();
        state.stack.push(vec![]);
        let mut buf = vec![];
        let len = state.clone().to_compact(&mut buf);
        let (decoded, _) = HashBuilderState::from_compact(&buf, len);
        assert_eq!(state, decoded);
    }

    proptest! {
        #[test]
        fn hash_builder_state_roundtrip(state: HashBuilderState) {
            let mut buf = vec![];
            let len = state.clone().to_compact(&mut buf);
            let (decoded, _) = HashBuilderState::from_compact(&buf, len);
            assert_eq!(state, decoded);
        }
    }
}

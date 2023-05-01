use crate::{
    trie::{hash_builder::HashBuilderState, StoredSubNode},
    Address, H256,
};
use bytes::Buf;
use reth_codecs::{main_codec, Compact};

/// Saves the progress of Merkle stage.
#[derive(Default, Debug, Clone, PartialEq)]
pub struct MerkleCheckpoint {
    // TODO: target block?
    /// The last hashed account key processed.
    pub last_account_key: H256,
    /// The last walker key processed.
    pub last_walker_key: Vec<u8>,
    /// Previously recorded walker stack.
    pub walker_stack: Vec<StoredSubNode>,
    /// The hash builder state.
    pub state: HashBuilderState,
}

impl Compact for MerkleCheckpoint {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let mut len = 0;

        buf.put_slice(self.last_account_key.as_slice());
        len += self.last_account_key.len();

        buf.put_u16(self.last_walker_key.len() as u16);
        buf.put_slice(&self.last_walker_key[..]);
        len += 2 + self.last_walker_key.len();

        buf.put_u16(self.walker_stack.len() as u16);
        len += 2;
        for item in self.walker_stack.into_iter() {
            len += item.to_compact(buf);
        }

        len += self.state.to_compact(buf);
        len
    }

    fn from_compact(mut buf: &[u8], _len: usize) -> (Self, &[u8])
    where
        Self: Sized,
    {
        let last_account_key = H256::from_slice(&buf[..32]);
        buf.advance(32);

        let last_walker_key_len = buf.get_u16() as usize;
        let last_walker_key = Vec::from(&buf[..last_walker_key_len]);
        buf.advance(last_walker_key_len);

        let walker_stack_len = buf.get_u16() as usize;
        let mut walker_stack = Vec::with_capacity(walker_stack_len);
        for _ in 0..walker_stack_len {
            let (item, rest) = StoredSubNode::from_compact(buf, 0);
            walker_stack.push(item);
            buf = rest;
        }

        let (state, buf) = HashBuilderState::from_compact(buf, 0);
        (MerkleCheckpoint { last_account_key, last_walker_key, walker_stack, state }, buf)
    }
}

/// Saves the progress of AccountHashing
#[main_codec]
#[derive(Default, Debug, Copy, Clone, PartialEq)]
pub struct AccountHashingCheckpoint {
    /// The next account to start hashing from
    pub address: Option<Address>,
    /// Start transition id
    pub from: u64,
    /// Last transition id
    pub to: u64,
}

/// Saves the progress of StorageHashing
#[main_codec]
#[derive(Default, Debug, Copy, Clone, PartialEq)]
pub struct StorageHashingCheckpoint {
    /// The next account to start hashing from
    pub address: Option<Address>,
    /// The next storage slot to start hashing from
    pub storage: Option<H256>,
    /// Start transition id
    pub from: u64,
    /// Last transition id
    pub to: u64,
}

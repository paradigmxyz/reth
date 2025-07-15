/// MPT implementation based on RSP:
/// - https://github.com/succinctlabs/rsp/blob/4081e40833aebece5958518724a327ede100f6cd/crates/mpt/src/mpt.rs
///
extern crate alloc;

/// Root hash of an empty trie.
pub const EMPTY_ROOT: alloy_primitives::B256 =
  alloy_primitives::b256!("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421");

pub trait RlpBytes {
  /// Returns the RLP-encoding.
  fn to_rlp(&self) -> alloc::vec::Vec<u8>;
}

impl<T> RlpBytes for T
where
  T: alloy_rlp::Encodable,
{
  #[inline]
  fn to_rlp(&self) -> alloc::vec::Vec<u8> {
    let rlp_length = self.length();
    let mut out = alloc::vec::Vec::with_capacity(rlp_length);
    self.encode(&mut out);
    debug_assert_eq!(out.len(), rlp_length);
    out
  }
}

/// Computes the Keccak-256 hash of the provided data.
///
/// This function is a thin wrapper around the Keccak256 hashing algorithm
/// and is optimized for performance.
///
/// # TODO
/// - Consider switching the return type to `alloy_primitives::B256` for consistency with other parts of the codebase.
#[inline]
pub fn keccak(data: impl AsRef<[u8]>) -> [u8; 32] {
  // TODO: Remove this benchmarking code once performance testing is complete.
  // std::hint::black_box(sha2::Sha256::digest(&data));
  *alloy_primitives::utils::keccak256(data)
}

/// Represents the root node of a sparse Merkle Patricia Trie.
///
/// The "sparse" nature of this trie allows for truncation of certain unneeded parts,
/// representing them by their node hash. This design choice is particularly useful for
/// optimizing storage. However, operations targeting a truncated part will fail and
/// return an error. Another distinction of this implementation is that branches cannot
/// store values, aligning with the construction of MPTs in Ethereum.
#[derive(
  Clone, Debug, Default, PartialEq, Eq, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub struct MptNode {
  /// The type and data of the node.
  pub data: MptNodeData,
  /// Cache for a previously computed reference of this node. This is skipped during
  /// serialization.
  #[serde(skip)]
  pub cached_reference: core::cell::RefCell<Option<MptNodeReference>>,
}

/// Represents custom error types for the sparse Merkle Patricia Trie (MPT).
///
/// These errors cover various scenarios that can occur during trie operations, such as
/// encountering unresolved nodes, finding values in branches where they shouldn't be, and
/// issues related to RLP (Recursive Length Prefix) encoding and decoding.
#[derive(Debug)]
pub enum Error {
  /// Triggered when an operation reaches an unresolved node. The associated `alloy_primitives::B256`
  /// value provides details about the unresolved node.
  //#[error("reached an unresolved node: {0:#}")]
  NodeNotResolved(alloy_primitives::B256),
  /// Occurs when a value is unexpectedly found in a branch node.
  //#[error("branch node with value")]
  ValueInBranch,
  /// Represents errors related to the RLP encoding and decoding using the `alloy_rlp`
  /// library.
  //#[error("RLP error")]
  Rlp(alloy_rlp::Error),
  /// Represents errors related to the RLP encoding and decoding, specifically legacy
  /// errors.
  //#[error("RLP error")]
  LegacyRlp(rlp::DecoderError),
}

/// Represents the various types of data that can be stored within a node in the sparse
/// Merkle Patricia Trie (MPT).
///
/// Each node in the trie can be of one of several types, each with its own specific data
/// structure. This enum provides a clear and type-safe way to represent the data
/// associated with each node type.
#[derive(
  Clone, Debug, Default, PartialEq, Eq, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub enum MptNodeData {
  /// Represents an empty trie node.
  #[default]
  Null,
  /// A node that can have up to 16 children. Each child is an optional boxed [MptNode].
  Branch([Option<alloc::boxed::Box<MptNode>>; 16]),
  /// A leaf node that contains a key and a value, both represented as byte vectors.
  Leaf(alloc::vec::Vec<u8>, alloc::vec::Vec<u8>),
  /// A node that has exactly one child and is used to represent a shared prefix of
  /// several keys.
  Extension(alloc::vec::Vec<u8>, alloc::boxed::Box<MptNode>),
  /// Represents a sub-trie by its hash, allowing for efficient storage of large
  /// sub-tries without storing their entire content.
  Digest(alloy_primitives::B256),
}

/// Represents the ways in which one node can reference another node inside the sparse
/// Merkle Patricia Trie (MPT).
///
/// Nodes in the MPT can reference other nodes either directly through their byte
/// representation or indirectly through a hash of their encoding. This enum provides a
/// clear and type-safe way to represent these references.
#[derive(
  Clone, Debug, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub enum MptNodeReference {
  /// Represents a direct reference to another node using its byte encoding. Typically
  /// used for short encodings that are less than 32 bytes in length.
  Bytes(alloc::vec::Vec<u8>),
  /// Represents an indirect reference to another node using the Keccak hash of its long
  /// encoding. Used for encodings that are not less than 32 bytes in length.
  Digest(alloy_primitives::B256),
}

/// Provides encoding functionalities for the `MptNode` type.
///
/// This implementation allows for the serialization of an [MptNode] into its RLP-encoded
/// form. The encoding is done based on the type of node data ([MptNodeData]) it holds.
impl alloy_rlp::Encodable for MptNode {
  /// Encodes the node into the provided `out` buffer.
  ///
  /// The encoding is done using the Recursive Length Prefix (RLP) encoding scheme. The
  /// method handles different node data types and encodes them accordingly.
  #[inline]
  fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
    match &self.data {
      MptNodeData::Null => {
        out.put_u8(alloy_rlp::EMPTY_STRING_CODE);
      }
      MptNodeData::Branch(nodes) => {
        alloy_rlp::Header {
          list: true,
          payload_length: self.payload_length(),
        }
        .encode(out);
        nodes.iter().for_each(|child| match child {
          Some(node) => node.reference_encode(out),
          None => out.put_u8(alloy_rlp::EMPTY_STRING_CODE),
        });
        // in the MPT reference, branches have values so always add empty value
        out.put_u8(alloy_rlp::EMPTY_STRING_CODE);
      }
      MptNodeData::Leaf(prefix, value) => {
        alloy_rlp::Header {
          list: true,
          payload_length: self.payload_length(),
        }
        .encode(out);
        prefix.as_slice().encode(out);
        value.as_slice().encode(out);
      }
      MptNodeData::Extension(prefix, node) => {
        alloy_rlp::Header {
          list: true,
          payload_length: self.payload_length(),
        }
        .encode(out);
        prefix.as_slice().encode(out);
        node.reference_encode(out);
      }
      MptNodeData::Digest(digest) => {
        digest.encode(out);
      }
    }
  }

  /// Returns the length of the encoded node in bytes.
  ///
  /// This method calculates the length of the RLP-encoded node. It's useful for
  /// determining the size requirements for storage or transmission.
  #[inline]
  fn length(&self) -> usize {
    let payload_length = self.payload_length();
    payload_length + alloy_rlp::length_of_length(payload_length)
  }
}

/// Provides decoding functionalities for the [MptNode] type.
///
/// This implementation allows for the deserialization of an RLP-encoded [MptNode] back
/// into its original form. The decoding is done based on the prototype of the RLP data,
/// ensuring that the node is reconstructed accurately.
///
/// **Note**: This implementation is still using the older RLP library and needs to be
/// migrated to `alloy_rlp` in the future.
impl rlp::Decodable for MptNode {
  /// Decodes an RLP-encoded node from the provided `rlp` buffer.
  ///
  /// The method handles different RLP prototypes and reconstructs the `MptNode` based
  /// on the encoded data. If the RLP data does not match any known prototype or if
  /// there's an error during decoding, an error is returned.
  fn decode(rlp: &rlp::Rlp<'_>) -> Result<Self, rlp::DecoderError> {
    match rlp.prototype()? {
      rlp::Prototype::Null | rlp::Prototype::Data(0) => Ok(MptNodeData::Null.into()),
      rlp::Prototype::List(2) => {
        let path: alloc::vec::Vec<u8> = rlp.val_at(0)?;
        let prefix = path[0];
        if (prefix & (2 << 4)) == 0 {
          let node: MptNode = rlp::Decodable::decode(&rlp.at(1)?)?;
          Ok(MptNodeData::Extension(path, alloc::boxed::Box::new(node)).into())
        } else {
          Ok(MptNodeData::Leaf(path, rlp.val_at(1)?).into())
        }
      }
      rlp::Prototype::List(17) => {
        let mut node_list = alloc::vec::Vec::with_capacity(16);
        for node_rlp in rlp.iter().take(16) {
          match node_rlp.prototype()? {
            rlp::Prototype::Null | rlp::Prototype::Data(0) => {
              node_list.push(None);
            }
            _ => node_list.push(Some(alloc::boxed::Box::new(rlp::Decodable::decode(
              &node_rlp,
            )?))),
          }
        }
        let value: alloc::vec::Vec<u8> = rlp.val_at(16)?;
        if value.is_empty() {
          Ok(MptNodeData::Branch(node_list.try_into().unwrap()).into())
        } else {
          Err(rlp::DecoderError::Custom("branch node with value"))
        }
      }
      rlp::Prototype::Data(32) => {
        let bytes: alloc::vec::Vec<u8> = rlp.as_val()?;
        Ok(MptNodeData::Digest(alloy_primitives::B256::from_slice(&bytes)).into())
      }
      _ => Err(rlp::DecoderError::RlpIncorrectListLen),
    }
  }
}

/// Provides a conversion from [MptNodeData] to [MptNode].
///
/// This implementation allows for conversion from [MptNodeData] to [MptNode],
/// initializing the `data` field with the provided value and setting the
/// `cached_reference` field to `None`.
impl From<MptNodeData> for MptNode {
  fn from(value: MptNodeData) -> Self {
    Self {
      data: value,
      cached_reference: core::cell::RefCell::new(None),
    }
  }
}

/// Represents a node in the sparse Merkle Patricia Trie (MPT).
///
/// The [MptNode] type encapsulates the data and functionalities associated with a node in
/// the MPT. It provides methods for manipulating the trie, such as inserting, deleting,
/// and retrieving values, as well as utility methods for encoding, decoding, and
/// debugging.
impl MptNode {
  /// Clears the trie, replacing its data with an empty node, [MptNodeData::Null].
  ///
  /// This method effectively removes all key-value pairs from the trie.
  #[inline]
  pub fn clear(&mut self) {
    self.data = MptNodeData::Null;
    self.invalidate_ref_cache();
  }

  /// Decodes an RLP-encoded [MptNode] from the provided byte slice.
  ///
  /// This method allows for the deserialization of a previously serde::Serialized [MptNode].
  #[inline]
  pub fn decode(bytes: impl AsRef<[u8]>) -> Result<MptNode, Error> {
    rlp::decode(bytes.as_ref()).map_err(|e| Error::LegacyRlp(e))
  }

  /// Retrieves the underlying data of the node.
  ///
  /// This method provides a reference to the node's data, allowing for inspection and
  /// manipulation.
  #[inline]
  pub fn as_data(&self) -> &MptNodeData {
    &self.data
  }

  /// Retrieves the [MptNodeReference] reference of the node when it's referenced inside
  /// another node.
  ///
  /// This method provides a way to obtain a compact representation of the node for
  /// storage or transmission purposes.
  #[inline]
  pub fn reference(&self) -> MptNodeReference {
    self
      .cached_reference
      .borrow_mut()
      .get_or_insert_with(|| self.calc_reference())
      .clone()
  }

  /// Computes and returns the 256-bit hash of the node.
  ///
  /// This method provides a unique identifier for the node based on its content.
  #[inline]
  pub fn hash(&self) -> alloy_primitives::B256 {
    match self.data {
      MptNodeData::Null => EMPTY_ROOT,
      _ => match self
        .cached_reference
        .borrow_mut()
        .get_or_insert_with(|| self.calc_reference())
      {
        MptNodeReference::Digest(digest) => *digest,
        MptNodeReference::Bytes(bytes) => keccak(bytes).into(),
      },
    }
  }

  /// Encodes the [MptNodeReference] of this node into the `out` buffer.
  fn reference_encode(&self, out: &mut dyn alloy_rlp::BufMut) {
    match self
      .cached_reference
      .borrow_mut()
      .get_or_insert_with(|| self.calc_reference())
    {
      // if the reference is an RLP-encoded byte slice, copy it directly
      MptNodeReference::Bytes(bytes) => out.put_slice(bytes),
      // if the reference is a digest, RLP-encode it with its fixed known length
      MptNodeReference::Digest(digest) => {
        out.put_u8(alloy_rlp::EMPTY_STRING_CODE + 32);
        out.put_slice(digest.as_slice());
      }
    }
  }

  /// Returns the length of the encoded [MptNodeReference] of this node.
  fn reference_length(&self) -> usize {
    match self
      .cached_reference
      .borrow_mut()
      .get_or_insert_with(|| self.calc_reference())
    {
      MptNodeReference::Bytes(bytes) => bytes.len(),
      MptNodeReference::Digest(_) => 1 + 32,
    }
  }

  fn calc_reference(&self) -> MptNodeReference {
    match &self.data {
      MptNodeData::Null => MptNodeReference::Bytes(alloc::vec![alloy_rlp::EMPTY_STRING_CODE]),
      MptNodeData::Digest(digest) => MptNodeReference::Digest(*digest),
      _ => {
        let encoded = alloy_rlp::encode(self);
        if encoded.len() < 32 {
          MptNodeReference::Bytes(encoded)
        } else {
          MptNodeReference::Digest(keccak(encoded).into())
        }
      }
    }
  }

  /// Determines if the trie is empty.
  ///
  /// This method checks if the node represents an empty trie, i.e., it doesn't contain
  /// any key-value pairs.
  #[inline]
  pub fn is_empty(&self) -> bool {
    matches!(&self.data, MptNodeData::Null)
  }

  /// Determines if the node represents a digest.
  ///
  /// A digest is a compact representation of a sub-trie, represented by its hash.
  #[inline]
  pub fn is_digest(&self) -> bool {
    matches!(&self.data, MptNodeData::Digest(_))
  }

  /// Retrieves the nibbles corresponding to the node's prefix.
  ///
  /// Nibbles are half-bytes, and in the context of the MPT, they represent parts of
  /// keys.
  #[inline]
  pub fn nibs(&self) -> alloc::vec::Vec<u8> {
    match &self.data {
      MptNodeData::Null | MptNodeData::Branch(_) | MptNodeData::Digest(_) => alloc::vec![],
      MptNodeData::Leaf(prefix, _) | MptNodeData::Extension(prefix, _) => prefix_nibs(prefix),
    }
  }

  /// Retrieves the value associated with a given key in the trie.
  ///
  /// If the key is not present in the trie, this method returns `None`. Otherwise, it
  /// returns a reference to the associated value. If [None] is returned, the key is
  /// provably not in the trie.
  #[inline]
  pub fn get(&self, key: &[u8]) -> Result<Option<&[u8]>, Error> {
    self.get_internal(&to_nibs(key))
  }

  /// Retrieves the RLP-decoded value corresponding to the key.
  ///
  /// If the key is not present in the trie, this method returns `None`. Otherwise, it
  /// returns the RLP-decoded value.
  #[inline]
  pub fn get_rlp<T: alloy_rlp::Decodable>(&self, key: &[u8]) -> Result<Option<T>, Error> {
    match self.get(key)? {
      Some(mut bytes) => Ok(Some(T::decode(&mut bytes).map_err(|e| Error::Rlp(e))?)),
      None => Ok(None),
    }
  }

  fn get_internal(&self, key_nibs: &[u8]) -> Result<Option<&[u8]>, Error> {
    match &self.data {
      MptNodeData::Null => Ok(None),
      MptNodeData::Branch(nodes) => {
        if let Some((i, tail)) = key_nibs.split_first() {
          match nodes[*i as usize] {
            Some(ref node) => node.get_internal(tail),
            None => Ok(None),
          }
        } else {
          Ok(None)
        }
      }
      MptNodeData::Leaf(prefix, value) => {
        if prefix_nibs(prefix) == key_nibs {
          Ok(Some(value))
        } else {
          Ok(None)
        }
      }
      MptNodeData::Extension(prefix, node) => {
        if let Some(tail) = key_nibs.strip_prefix(prefix_nibs(prefix).as_slice()) {
          node.get_internal(tail)
        } else {
          Ok(None)
        }
      }
      MptNodeData::Digest(digest) => Err(Error::NodeNotResolved(*digest)),
    }
  }

  /// Removes a key from the trie.
  ///
  /// This method attempts to remove a key-value pair from the trie. If the key is
  /// present, it returns `true`. Otherwise, it returns `false`.
  #[inline]
  pub fn delete(&mut self, key: &[u8]) -> Result<bool, Error> {
    self.delete_internal(&to_nibs(key))
  }

  fn delete_internal(&mut self, key_nibs: &[u8]) -> Result<bool, Error> {
    match &mut self.data {
      MptNodeData::Null => return Ok(false),
      MptNodeData::Branch(children) => {
        if let Some((i, tail)) = key_nibs.split_first() {
          let child = &mut children[*i as usize];
          match child {
            Some(node) => {
              if !node.delete_internal(tail)? {
                return Ok(false);
              }
              // if the node is now empty, remove it
              if node.is_empty() {
                *child = None;
              }
            }
            None => return Ok(false),
          }
        } else {
          return Err(Error::ValueInBranch);
        }

        let mut remaining = children.iter_mut().enumerate().filter(|(_, n)| n.is_some());
        // there will always be at least one remaining node
        let (index, node) = remaining.next().unwrap();
        // if there is only exactly one node left, we need to convert the branch
        if remaining.next().is_none() {
          let mut orphan = node.take().unwrap();
          match &mut orphan.data {
            // if the orphan is a leaf, prepend the corresponding nib to it
            MptNodeData::Leaf(prefix, orphan_value) => {
              let new_nibs: alloc::vec::Vec<_> = core::iter::once(index as u8)
                .chain(prefix_nibs(prefix))
                .collect();
              self.data = MptNodeData::Leaf(
                to_encoded_path(&new_nibs, true),
                core::mem::take(orphan_value),
              );
            }
            // if the orphan is an extension, prepend the corresponding nib to it
            MptNodeData::Extension(prefix, orphan_child) => {
              let new_nibs: alloc::vec::Vec<_> = core::iter::once(index as u8)
                .chain(prefix_nibs(prefix))
                .collect();
              self.data = MptNodeData::Extension(
                to_encoded_path(&new_nibs, false),
                core::mem::take(orphan_child),
              );
            }
            // if the orphan is a branch or digest, convert to an extension
            MptNodeData::Branch(_) | MptNodeData::Digest(_) => {
              self.data = MptNodeData::Extension(to_encoded_path(&[index as u8], false), orphan);
            }
            MptNodeData::Null => unreachable!(),
          }
        }
      }
      MptNodeData::Leaf(prefix, _) => {
        if prefix_nibs(prefix) != key_nibs {
          return Ok(false);
        }
        self.data = MptNodeData::Null;
      }
      MptNodeData::Extension(prefix, child) => {
        let mut self_nibs = prefix_nibs(prefix);
        if let Some(tail) = key_nibs.strip_prefix(self_nibs.as_slice()) {
          if !child.delete_internal(tail)? {
            return Ok(false);
          }
        } else {
          return Ok(false);
        }

        // an extension can only point to a branch or a digest; since it's sub trie was
        // modified, we need to make sure that this property still holds
        match &mut child.data {
          // if the child is empty, remove the extension
          MptNodeData::Null => {
            self.data = MptNodeData::Null;
          }
          // for a leaf, replace the extension with the extended leaf
          MptNodeData::Leaf(prefix, value) => {
            self_nibs.extend(prefix_nibs(prefix));
            self.data =
              MptNodeData::Leaf(to_encoded_path(&self_nibs, true), core::mem::take(value));
          }
          // for an extension, replace the extension with the extended extension
          MptNodeData::Extension(prefix, node) => {
            self_nibs.extend(prefix_nibs(prefix));
            self.data =
              MptNodeData::Extension(to_encoded_path(&self_nibs, false), core::mem::take(node));
          }
          // for a branch or digest, the extension is still correct
          MptNodeData::Branch(_) | MptNodeData::Digest(_) => {}
        }
      }
      MptNodeData::Digest(digest) => return Err(Error::NodeNotResolved(*digest)),
    };

    self.invalidate_ref_cache();
    Ok(true)
  }

  /// Inserts a key-value pair into the trie.
  ///
  /// This method attempts to insert a new key-value pair into the trie. If the
  /// insertion is successful, it returns `true`. If the key already exists, it updates
  /// the value and returns `false`.
  #[inline]
  pub fn insert(&mut self, key: &[u8], value: alloc::vec::Vec<u8>) -> Result<bool, Error> {
    if value.is_empty() {
      panic!("value must not be empty");
    }
    self.insert_internal(&to_nibs(key), value)
  }

  /// Inserts an RLP-encoded value into the trie.
  ///
  /// This method inserts a value that's been encoded using RLP into the trie.
  #[inline]
  pub fn insert_rlp(
    &mut self,
    key: &[u8],
    value: impl alloy_rlp::Encodable,
  ) -> Result<bool, Error> {
    self.insert_internal(&to_nibs(key), value.to_rlp())
  }

  fn insert_internal(
    &mut self,
    key_nibs: &[u8],
    value: alloc::vec::Vec<u8>,
  ) -> Result<bool, Error> {
    match &mut self.data {
      MptNodeData::Null => {
        self.data = MptNodeData::Leaf(to_encoded_path(key_nibs, true), value);
      }
      MptNodeData::Branch(children) => {
        if let Some((i, tail)) = key_nibs.split_first() {
          let child = &mut children[*i as usize];
          match child {
            Some(node) => {
              if !node.insert_internal(tail, value)? {
                return Ok(false);
              }
            }
            // if the corresponding child is empty, insert a new leaf
            None => {
              *child = Some(alloc::boxed::Box::new(
                MptNodeData::Leaf(to_encoded_path(tail, true), value).into(),
              ));
            }
          }
        } else {
          return Err(Error::ValueInBranch);
        }
      }
      MptNodeData::Leaf(prefix, old_value) => {
        let self_nibs = prefix_nibs(prefix);
        let common_len = lcp(&self_nibs, key_nibs);
        if common_len == self_nibs.len() && common_len == key_nibs.len() {
          // if self_nibs == key_nibs, update the value if it is different
          if old_value == &value {
            return Ok(false);
          }
          *old_value = value;
        } else if common_len == self_nibs.len() || common_len == key_nibs.len() {
          return Err(Error::ValueInBranch);
        } else {
          let split_point = common_len + 1;
          // otherwise, create a branch with two children
          let mut children: [Option<alloc::boxed::Box<MptNode>>; 16] = Default::default();

          children[self_nibs[common_len] as usize] = Some(alloc::boxed::Box::new(
            MptNodeData::Leaf(
              to_encoded_path(&self_nibs[split_point..], true),
              core::mem::take(old_value),
            )
            .into(),
          ));
          children[key_nibs[common_len] as usize] = Some(alloc::boxed::Box::new(
            MptNodeData::Leaf(to_encoded_path(&key_nibs[split_point..], true), value).into(),
          ));

          let branch = MptNodeData::Branch(children);
          if common_len > 0 {
            // create parent extension for new branch
            self.data = MptNodeData::Extension(
              to_encoded_path(&self_nibs[..common_len], false),
              alloc::boxed::Box::new(branch.into()),
            );
          } else {
            self.data = branch;
          }
        }
      }
      MptNodeData::Extension(prefix, existing_child) => {
        let self_nibs = prefix_nibs(prefix);
        let common_len = lcp(&self_nibs, key_nibs);
        if common_len == self_nibs.len() {
          // traverse down for update
          if !existing_child.insert_internal(&key_nibs[common_len..], value)? {
            return Ok(false);
          }
        } else if common_len == key_nibs.len() {
          return Err(Error::ValueInBranch);
        } else {
          let split_point = common_len + 1;
          // otherwise, create a branch with two children
          let mut children: [Option<alloc::boxed::Box<MptNode>>; 16] = Default::default();

          children[self_nibs[common_len] as usize] = if split_point < self_nibs.len() {
            Some(alloc::boxed::Box::new(
              MptNodeData::Extension(
                to_encoded_path(&self_nibs[split_point..], false),
                core::mem::take(existing_child),
              )
              .into(),
            ))
          } else {
            Some(core::mem::take(existing_child))
          };
          children[key_nibs[common_len] as usize] = Some(alloc::boxed::Box::new(
            MptNodeData::Leaf(to_encoded_path(&key_nibs[split_point..], true), value).into(),
          ));

          let branch = MptNodeData::Branch(children);
          if common_len > 0 {
            // Create parent extension for new branch
            self.data = MptNodeData::Extension(
              to_encoded_path(&self_nibs[..common_len], false),
              alloc::boxed::Box::new(branch.into()),
            );
          } else {
            self.data = branch;
          }
        }
      }
      MptNodeData::Digest(digest) => return Err(Error::NodeNotResolved(*digest)),
    };

    self.invalidate_ref_cache();
    Ok(true)
  }

  fn invalidate_ref_cache(&mut self) {
    self.cached_reference.borrow_mut().take();
  }

  /// Returns the number of traversable nodes in the trie.
  ///
  /// This method provides a count of all the nodes that can be traversed within the
  /// trie.
  pub fn size(&self) -> usize {
    match self.as_data() {
      MptNodeData::Null => 0,
      MptNodeData::Branch(children) => {
        children.iter().flatten().map(|n| n.size()).sum::<usize>() + 1
      }
      MptNodeData::Leaf(_, _) => 1,
      MptNodeData::Extension(_, child) => child.size() + 1,
      MptNodeData::Digest(_) => 0,
    }
  }

  /// Returns the length of the RLP payload of the node.
  fn payload_length(&self) -> usize {
    match &self.data {
      MptNodeData::Null => 0,
      MptNodeData::Branch(nodes) => {
        1 + nodes
          .iter()
          .map(|child| child.as_ref().map_or(1, |node| node.reference_length()))
          .sum::<usize>()
      }
      MptNodeData::Leaf(prefix, value) => {
        alloy_rlp::Encodable::length(&prefix.as_slice())
          + alloy_rlp::Encodable::length(&value.as_slice())
      }
      MptNodeData::Extension(prefix, node) => {
        alloy_rlp::Encodable::length(&prefix.as_slice()) + node.reference_length()
      }
      MptNodeData::Digest(_) => 32,
    }
  }
}

/// Converts a byte slice into a vector of nibbles.
///
/// A nibble is 4 bits or half of an 8-bit byte. This function takes each byte from the
/// input slice, splits it into two nibbles, and appends them to the resulting vector.
pub fn to_nibs(slice: &[u8]) -> alloc::vec::Vec<u8> {
  let mut result = alloc::vec::Vec::with_capacity(2 * slice.len());
  for byte in slice {
    result.push(byte >> 4);
    result.push(byte & 0xf);
  }
  result
}

/// Encodes a slice of nibbles into a vector of bytes, with an additional prefix to
/// indicate the type of node (leaf or extension).
///
/// The function starts by determining the type of node based on the `is_leaf` parameter.
/// If the node is a leaf, the prefix is set to `0x20`. If the length of the nibbles is
/// odd, the prefix is adjusted and the first nibble is incorporated into it.
///
/// The remaining nibbles are then combined into bytes, with each pair of nibbles forming
/// a single byte. The resulting vector starts with the prefix, followed by the encoded
/// bytes.
pub fn to_encoded_path(mut nibs: &[u8], is_leaf: bool) -> alloc::vec::Vec<u8> {
  let mut prefix = (is_leaf as u8) * 0x20;
  if nibs.len() % 2 != 0 {
    prefix += 0x10 + nibs[0];
    nibs = &nibs[1..];
  }
  core::iter::once(prefix)
    .chain(nibs.chunks_exact(2).map(|byte| (byte[0] << 4) + byte[1]))
    .collect()
}

/// Returns the length of the common prefix.
fn lcp(a: &[u8], b: &[u8]) -> usize {
  for (i, (a, b)) in core::iter::zip(a, b).enumerate() {
    if a != b {
      return i;
    }
  }
  core::cmp::min(a.len(), b.len())
}

fn prefix_nibs(prefix: &[u8]) -> alloc::vec::Vec<u8> {
  let (extension, tail) = prefix.split_first().unwrap();
  // the first bit of the first nibble denotes the parity
  let is_odd = extension & (1 << 4) != 0;

  let mut result = alloc::vec::Vec::with_capacity(2 * tail.len() + is_odd as usize);
  // for odd lengths, the second nibble contains the first element
  if is_odd {
    result.push(extension & 0xf);
  }
  for nib in tail {
    result.push(nib >> 4);
    result.push(nib & 0xf);
  }
  result
}

/// Creates a new MPT trie where all the digests contained in `node_store` are resolved.
pub fn resolve_nodes(
  root: &MptNode,
  node_store: &alloy_primitives::map::HashMap<MptNodeReference, MptNode>,
) -> MptNode {
  let trie = match root.as_data() {
    MptNodeData::Null | MptNodeData::Leaf(_, _) => root.clone(),
    MptNodeData::Branch(children) => {
      let children: alloc::vec::Vec<_> = children
        .iter()
        .map(|child| {
          child
            .as_ref()
            .map(|node| alloc::boxed::Box::new(resolve_nodes(node, node_store)))
        })
        .collect();
      MptNodeData::Branch(children.try_into().unwrap()).into()
    }
    MptNodeData::Extension(prefix, target) => MptNodeData::Extension(
      prefix.clone(),
      alloc::boxed::Box::new(resolve_nodes(target, node_store)),
    )
    .into(),
    MptNodeData::Digest(digest) => {
      if let Some(node) = node_store.get(&MptNodeReference::Digest(*digest)) {
        resolve_nodes(node, node_store)
      } else {
        root.clone()
      }
    }
  };
  // the root hash must not change
  debug_assert_eq!(root.hash(), trie.hash());

  trie
}

/// Creates a new MPT trie where all the digests contained in `node_store` are resolved.
/// NOTE: We had to duplicate resolve_nodes function, as only *state* leafs has TrieAccount inside.
/// Originally RSP's resolve_nodes was used both for state and storage tries.
pub fn resolve_state_nodes(
  root: &MptNode,
  node_store: &alloy_primitives::map::HashMap<MptNodeReference, MptNode>,
  storage_roots_detected: &mut alloc::vec::Vec<(
    alloy_primitives::FixedBytes<32>,
    alloy_primitives::FixedBytes<32>,
  )>,
  path: nybbles::Nibbles,
) -> MptNode {
  let trie = match root.as_data() {
    MptNodeData::Null => root.clone(),
    MptNodeData::Leaf(_key, _value) => {
      // We need to track address path for storage roots!
      let mut full_path = path.clone();
      // Convert _key to nibbles, as it is a leaf.
      let key_nibs = prefix_nibs(_key);
      full_path.extend_from_slice_unchecked(&key_nibs);
      let hashed_address = alloy_primitives::B256::from_slice(&full_path.pack());

      let account =
        <alloy_trie::TrieAccount as alloy_rlp::Decodable>::decode(&mut &_value[..]).unwrap();
      if account.storage_root != alloy_trie::EMPTY_ROOT_HASH {
        storage_roots_detected.push((hashed_address, account.storage_root));
      }

      root.clone()
    }
    MptNodeData::Branch(children) => {
      let children: alloc::vec::Vec<_> = children
        .iter()
        .enumerate()
        .map(|(idx, child)| {
          child.as_ref().map(|node| {
            let mut child_path = path.clone();
            child_path.push_unchecked(idx as u8);
            alloc::boxed::Box::new(resolve_state_nodes(
              node,
              node_store,
              storage_roots_detected,
              child_path,
            ))
          })
        })
        .collect();
      MptNodeData::Branch(children.try_into().unwrap()).into()
    }
    MptNodeData::Extension(prefix, target) => {
      let mut child_path = path.clone();
      let prefix_nibs = prefix_nibs(prefix);
      child_path.extend_from_slice_unchecked(&prefix_nibs);
      MptNodeData::Extension(
        prefix.clone(),
        alloc::boxed::Box::new(resolve_state_nodes(
          target,
          node_store,
          storage_roots_detected,
          child_path,
        )),
      )
      .into()
    }
    MptNodeData::Digest(digest) => {
      if let Some(node) = node_store.get(&MptNodeReference::Digest(*digest)) {
        resolve_state_nodes(node, node_store, storage_roots_detected, path)
      } else {
        root.clone()
      }
    }
  };
  // the root hash must not change
  debug_assert_eq!(root.hash(), trie.hash());

  trie
}

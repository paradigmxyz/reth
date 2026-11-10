//! EIP-8347 tagged snapshot/preimage artifacts and dual-check authentication.
//!
//! This implementation keeps the state in memory. It is suitable for conformance testing and
//! devnet checkpoints; mainnet-scale conversion needs an external-sort, streaming backend.

use crate::{key, AccountData, StateError, StateTree, TreeHasher};
use alloy_primitives::{keccak256, Address, Bytes, B256, U256};
use alloy_trie::{
    root::{state_root_unhashed, storage_root_unhashed},
    TrieAccount,
};
use std::collections::BTreeMap;

/// A pair of byte-canonical snapshot and preimage files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artifacts {
    /// Tagged snapshot records, end tag `0x07`, and the claimed PBT root.
    pub snapshot: Vec<u8>,
    /// Account/slot preimages ordered by their MPT hashes.
    pub preimages: Vec<u8>,
}

impl Artifacts {
    /// Serializes a complete anchor state satisfying EIP-7523 into canonical artifacts.
    pub fn from_state<H: TreeHasher>(state: &StateTree<H>) -> Result<Self, ArtifactError> {
        let mut snapshot = Vec::new();
        let mut headers: Vec<_> = state.accounts().collect();
        headers.sort_unstable_by_key(|(address, _)| key::account_hash::<H>(*address));
        for (address, account) in &headers {
            if account.is_empty() {
                return Err(ArtifactError::Invalid("empty anchor account"));
            }
            let kind = if account.is_delegation() { 2 } else { u8::from(!account.code.is_empty()) };
            snapshot.push(kind);
            snapshot.extend_from_slice(key::account_hash::<H>(*address).as_slice());
            put_integer(&mut snapshot, &account.nonce.to_be_bytes());
            put_integer(&mut snapshot, &account.balance.to_be_bytes::<32>());
            match kind {
                1 => {
                    snapshot.extend_from_slice(keccak256(&account.code).as_slice());
                    put_integer(&mut snapshot, &(account.code.len() as u32).to_be_bytes());
                }
                2 => snapshot.extend_from_slice(&account.code[3..]),
                _ => {}
            }
            let mut inline: Vec<_> = state
                .storage_entries(*address)
                .filter(|(slot, _)| *slot < U256::from(64))
                .collect();
            inline.sort_unstable_by_key(|(slot, _)| *slot);
            snapshot.push(inline.len() as u8);
            for (slot, value) in inline {
                snapshot.push(slot.to::<u8>());
                put_integer(&mut snapshot, value.as_slice());
            }
        }
        let mut groups: BTreeMap<Vec<u8>, Vec<(u8, B256)>> = BTreeMap::new();
        for (key, value) in state.tree().entries().filter(|(key, _)| key[0] != key::ACCOUNT_ZONE) {
            groups
                .entry(key[..key.len() - 1].to_vec())
                .or_default()
                .push((key[key.len() - 1], value));
        }
        let mut storage_account: Option<B256> = None;
        for (stem, entries) in groups {
            if stem[0] == key::CODE_ZONE {
                snapshot.push(3);
                snapshot.extend_from_slice(&stem[1..]);
                snapshot.push((entries.len() - 1) as u8);
            } else {
                let address_hash = B256::from_slice(&stem[1..33]);
                if storage_account != Some(address_hash) {
                    snapshot.push(4);
                    snapshot.extend_from_slice(address_hash.as_slice());
                    storage_account = Some(address_hash);
                }
                snapshot.push(if entries.len() == 1 { 5 } else { 6 });
                snapshot.extend_from_slice(&stem[33..]);
                if entries.len() > 1 {
                    snapshot.push((entries.len() - 1) as u8);
                }
            }
            for (index, value) in entries {
                snapshot.push(index);
                put_integer(&mut snapshot, value.as_slice());
            }
        }
        snapshot.push(7);
        snapshot.extend_from_slice(state.root().as_slice());

        let mut preimages = Vec::new();
        headers.sort_unstable_by_key(|(address, _)| keccak256(address));
        for (address, _) in headers {
            preimages.extend_from_slice(address.as_slice());
            let mut slots: Vec<_> =
                state.storage_entries(address).map(|(slot, _)| B256::from(slot)).collect();
            slots.sort_unstable_by_key(|slot| keccak256(slot));
            let count = u32::try_from(slots.len()).map_err(|_| ArtifactError::ResourceLimit)?;
            preimages.extend_from_slice(&count.to_be_bytes());
            for slot in slots {
                preimages.extend_from_slice(slot.as_slice());
            }
        }
        Ok(Self { snapshot, preimages })
    }

    /// Keccak digests of the complete files, useful for producer agreement, not authentication.
    pub fn digests(&self) -> (B256, B256) {
        (keccak256(&self.snapshot), keccak256(&self.preimages))
    }

    /// Rebuilds both commitments and authenticates against a trusted anchor MPT root.
    ///
    /// Validates code hashes, code sizes, PUSH metadata, exact preimage coverage, canonical
    /// ordering/integers, and absence of surplus leaves. Neither artifact digest is trusted.
    pub fn verify<H: TreeHasher>(
        &self,
        anchor_mpt_root: B256,
        limits: DecodeLimits,
    ) -> Result<StateTree<H>, ArtifactError> {
        let state = self.decode::<H>(limits)?;
        if mpt_root(&state) != anchor_mpt_root {
            return Err(ArtifactError::AnchorRootMismatch);
        }
        Ok(state)
    }

    /// Restores a local checkpoint against a trusted PBT root, rather than an MPT anchor.
    pub fn restore<H: TreeHasher>(
        &self,
        trusted_pbt_root: B256,
        limits: DecodeLimits,
    ) -> Result<StateTree<H>, ArtifactError> {
        let state = self.decode::<H>(limits)?;
        if state.root() != trusted_pbt_root {
            return Err(ArtifactError::PbtRootMismatch);
        }
        Ok(state)
    }

    fn decode<H: TreeHasher>(&self, limits: DecodeLimits) -> Result<StateTree<H>, ArtifactError> {
        let (headers, leaves, claimed_root) = parse_snapshot(&self.snapshot, limits)?;
        let state = decode_state::<H>(&headers, &leaves, &self.preimages, claimed_root, limits)?;
        if Self::from_state(&state)? != *self {
            return Err(ArtifactError::Invalid("noncanonical artifact encoding"));
        }
        Ok(state)
    }
}

fn decode_state<H: TreeHasher>(
    headers: &BTreeMap<B256, Header>,
    leaves: &Leaves,
    preimages: &[u8],
    claimed_root: B256,
    limits: DecodeLimits,
) -> Result<StateTree<H>, ArtifactError> {
    let mut preimages = Reader(preimages);
    let mut state = StateTree::<H>::new();
    let mut previous_account = None;
    let mut codes: BTreeMap<B256, Bytes> = BTreeMap::new();
    let mut total_code_size = 0usize;
    let mut accounts = 0;
    while !preimages.0.is_empty() {
        accounts += 1;
        if accounts > limits.max_accounts {
            return Err(ArtifactError::ResourceLimit);
        }
        let address = Address::from_slice(preimages.take(20)?);
        check_order(&mut previous_account, keccak256(address))?;
        let header = headers
            .get(&key::account_hash::<H>(address))
            .ok_or(ArtifactError::Invalid("surplus account preimage"))?;
        let count = u32::from_be_bytes(preimages.take(4)?.try_into().unwrap()) as usize;
        if count > limits.max_leaves || count > preimages.0.len() / 32 {
            return Err(ArtifactError::ResourceLimit);
        }
        let code = match &header.code {
            CodeRef::Empty => Bytes::new(),
            CodeRef::Delegation(target) => [vec![0xef, 1, 0], target.to_vec()].concat().into(),
            CodeRef::Contract(hash, size) => {
                if let Some(code) = codes.get(hash) {
                    if code.len() != *size {
                        return Err(ArtifactError::Invalid("inconsistent shared code size"));
                    }
                    code.clone()
                } else {
                    total_code_size =
                        total_code_size.checked_add(*size).ok_or(ArtifactError::ResourceLimit)?;
                    if total_code_size > limits.max_total_code_size {
                        return Err(ArtifactError::ResourceLimit);
                    }
                    let mut code = Vec::with_capacity(*size);
                    for index in 0..size.div_ceil(31) {
                        let chunk = leaves
                            .get(key::code_key::<H>(*hash, index as u64).as_slice())
                            .copied()
                            .unwrap_or_default();
                        let remaining = (*size - code.len()).min(31);
                        code.extend_from_slice(&chunk[1..1 + remaining]);
                    }
                    if keccak256(&code) != *hash {
                        return Err(ArtifactError::Invalid("code hash mismatch"));
                    }
                    let code: Bytes = code.into();
                    codes.insert(*hash, code.clone());
                    code
                }
            }
        };
        state.set_account(
            address,
            AccountData { nonce: header.nonce, balance: header.balance, code },
        )?;
        let mut previous_slot = None;
        for _ in 0..count {
            let slot = B256::from_slice(preimages.take(32)?);
            check_order(&mut previous_slot, keccak256(slot))?;
            let slot = U256::from_be_slice(slot.as_slice());
            let value = leaves
                .get(&key::storage_key::<H>(address, slot))
                .copied()
                .ok_or(ArtifactError::Invalid("surplus slot preimage"))?;
            state.set_storage(address, slot, value)?;
        }
    }
    // Equality of the exact leaf sets authenticates metadata and prevents surplus code,
    // storage in the wrong zone, noncanonical PUSH bytes, and unreferenced groups.
    if accounts != headers.len() ||
        !state.tree().entries().eq(leaves.iter().map(|(key, value)| (key.as_slice(), *value)))
    {
        return Err(ArtifactError::Invalid("snapshot and preimages do not describe the same state"));
    }
    if state.root() != claimed_root {
        return Err(ArtifactError::PbtRootMismatch);
    }
    Ok(state)
}

/// Consumer-controlled bounds for untrusted snapshot decoding.
#[derive(Clone, Copy, Debug)]
pub struct DecodeLimits {
    /// Maximum accounts allowed in one artifact.
    pub max_accounts: usize,
    /// Maximum leaves allowed in one artifact.
    pub max_leaves: usize,
    /// Maximum reconstructed bytecode size for one distinct contract.
    pub max_code_size: usize,
    /// Maximum total bytes reconstructed across distinct contract code hashes.
    pub max_total_code_size: usize,
}

impl Default for DecodeLimits {
    fn default() -> Self {
        Self {
            max_accounts: 1_000_000,
            max_leaves: 10_000_000,
            max_code_size: 16 * 1024 * 1024,
            max_total_code_size: 256 * 1024 * 1024,
        }
    }
}

/// Reconstructs the Ethereum MPT commitment from plain account/storage data.
pub fn mpt_root<H: TreeHasher>(state: &StateTree<H>) -> B256 {
    state_root_unhashed(state.accounts().map(|(address, account)| {
        let storage = storage_root_unhashed(
            state
                .storage_entries(address)
                .map(|(slot, value)| (B256::from(slot), U256::from_be_slice(value.as_slice()))),
        );
        (
            address,
            TrieAccount::new(account.nonce, account.balance, storage, keccak256(&account.code)),
        )
    }))
}

/// Rejected conversion artifact.
#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    /// Input ended inside a record or trailer.
    #[error("truncated PBT artifact")]
    Truncated,
    /// Malformed or noncanonical input.
    #[error("invalid PBT artifact: {0}")]
    Invalid(&'static str),
    /// A caller-selected resource bound was exceeded.
    #[error("PBT artifact exceeds decoding resource limits")]
    ResourceLimit,
    /// Rebuilt PBT root does not match the claimed or trusted root.
    #[error("PBT root mismatch")]
    PbtRootMismatch,
    /// Rebuilt MPT root does not match the trusted anchor header.
    #[error("anchor MPT root mismatch")]
    AnchorRootMismatch,
    /// Account data is not representable in PBT.
    #[error(transparent)]
    State(#[from] StateError),
}

#[derive(Debug)]
struct Header {
    nonce: u64,
    balance: U256,
    code: CodeRef,
}

#[derive(Debug)]
enum CodeRef {
    Empty,
    Contract(B256, usize),
    Delegation(Address),
}

type Leaves = BTreeMap<Vec<u8>, B256>;

fn parse_snapshot(
    bytes: &[u8],
    limits: DecodeLimits,
) -> Result<(BTreeMap<B256, Header>, Leaves, B256), ArtifactError> {
    let mut reader = Reader(bytes);
    let mut headers = BTreeMap::new();
    let mut leaves = BTreeMap::new();
    let mut storage_account: Option<B256> = None;
    let root = loop {
        let tag = reader.byte()?;
        match tag {
            0..=2 => {
                if headers.len() >= limits.max_accounts {
                    return Err(ArtifactError::ResourceLimit);
                }
                let address_hash = B256::from_slice(reader.take(32)?);
                let nonce = reader.integer(8)?.to::<u64>();
                let balance = reader.integer(16)?;
                let (code, size) = match tag {
                    0 => {
                        if nonce == 0 && balance.is_zero() {
                            return Err(ArtifactError::Invalid("empty anchor account"));
                        }
                        (CodeRef::Empty, 0)
                    }
                    1 => {
                        let hash = B256::from_slice(reader.take(32)?);
                        let size = reader.integer(4)?.to::<usize>();
                        if size == 0 {
                            return Err(ArtifactError::Invalid("zero contract code size"));
                        }
                        if size > limits.max_code_size {
                            return Err(ArtifactError::ResourceLimit);
                        }
                        (CodeRef::Contract(hash, size), size as u32)
                    }
                    _ => (CodeRef::Delegation(Address::from_slice(reader.take(20)?)), 23),
                };
                let mut stem = vec![key::ACCOUNT_ZONE];
                stem.extend_from_slice(address_hash.as_slice());
                let basic = key::encode_basic_data(size, nonce, balance)?;
                if !basic.is_zero() {
                    add_leaf(&mut leaves, &stem, 0, basic, limits)?;
                }
                match &code {
                    CodeRef::Empty => add_leaf(&mut leaves, &stem, 1, keccak256([]), limits)?,
                    CodeRef::Contract(hash, _) => add_leaf(&mut leaves, &stem, 1, *hash, limits)?,
                    CodeRef::Delegation(target) => {
                        let mut value = [0; 32];
                        value[..3].copy_from_slice(&[0xef, 1, 0]);
                        value[3..23].copy_from_slice(target.as_slice());
                        add_leaf(&mut leaves, &stem, 2, B256::from(value), limits)?;
                    }
                }
                let count = reader.byte()?;
                if count > 64 {
                    return Err(ArtifactError::Invalid("too many header slots"));
                }
                for _ in 0..count {
                    let slot = reader.byte()?;
                    if slot >= 64 {
                        return Err(ArtifactError::Invalid("invalid inline slot"));
                    }
                    let value = B256::from(reader.integer(32)?);
                    add_leaf(&mut leaves, &stem, 64 + slot, value, limits)?;
                }
                if headers.insert(address_hash, Header { nonce, balance, code }).is_some() {
                    return Err(ArtifactError::Invalid("duplicate header"));
                }
            }
            3 | 5 | 6 => {
                let mut stem = vec![if tag == 3 { key::CODE_ZONE } else { key::STORAGE_ZONE }];
                if tag != 3 {
                    let address_hash = storage_account
                        .ok_or(ArtifactError::Invalid("storage group without account"))?;
                    if !headers.contains_key(&address_hash) {
                        return Err(ArtifactError::Invalid("storage without header"));
                    }
                    stem.extend_from_slice(address_hash.as_slice());
                }
                stem.extend_from_slice(reader.take(32)?);
                let count = if tag == 5 { 1 } else { usize::from(reader.byte()?) + 1 };
                if tag == 6 && count == 1 {
                    return Err(ArtifactError::Invalid("noncanonical singleton group"));
                }
                for _ in 0..count {
                    let index = reader.byte()?;
                    let value = B256::from(reader.integer(32)?);
                    add_leaf(&mut leaves, &stem, index, value, limits)?;
                }
            }
            4 => storage_account = Some(B256::from_slice(reader.take(32)?)),
            7 => break B256::from_slice(reader.take(32)?),
            _ => return Err(ArtifactError::Invalid("unknown snapshot tag")),
        }
    };
    if !reader.0.is_empty() {
        return Err(ArtifactError::Invalid("trailing snapshot bytes"));
    }
    Ok((headers, leaves, root))
}

fn add_leaf(
    leaves: &mut Leaves,
    stem: &[u8],
    index: u8,
    value: B256,
    limits: DecodeLimits,
) -> Result<(), ArtifactError> {
    if leaves.len() >= limits.max_leaves {
        return Err(ArtifactError::ResourceLimit);
    }
    if value.is_zero() {
        return Err(ArtifactError::Invalid("zero-valued state leaf"));
    }
    let mut key = stem.to_vec();
    key.push(index);
    if leaves.insert(key, value).is_some() {
        return Err(ArtifactError::Invalid("duplicate leaf"));
    }
    Ok(())
}

pub(crate) fn check_order(previous: &mut Option<B256>, value: B256) -> Result<(), ArtifactError> {
    if previous.is_some_and(|previous| previous >= value) {
        return Err(ArtifactError::Invalid("preimage keys not strictly ordered"));
    }
    *previous = Some(value);
    Ok(())
}

fn put_integer(output: &mut Vec<u8>, bytes: &[u8]) {
    let first = bytes.iter().position(|byte| *byte != 0).unwrap_or(bytes.len());
    output.push((bytes.len() - first) as u8);
    output.extend_from_slice(&bytes[first..]);
}

pub(crate) struct Reader<'a>(pub(crate) &'a [u8]);

impl<'a> Reader<'a> {
    pub(crate) const fn take(&mut self, count: usize) -> Result<&'a [u8], ArtifactError> {
        if count > self.0.len() {
            return Err(ArtifactError::Truncated);
        }
        let (head, tail) = self.0.split_at(count);
        self.0 = tail;
        Ok(head)
    }
    fn byte(&mut self) -> Result<u8, ArtifactError> {
        Ok(self.take(1)?[0])
    }
    fn integer(&mut self, width: usize) -> Result<U256, ArtifactError> {
        let count = usize::from(self.byte()?);
        if count > width {
            return Err(ArtifactError::Invalid("integer exceeds field width"));
        }
        let bytes = self.take(count)?;
        if bytes.first() == Some(&0) {
            return Err(ArtifactError::Invalid("nonminimal integer"));
        }
        Ok(U256::from_be_slice(bytes))
    }

    #[cfg(any(feature = "conversion", feature = "persistence"))]
    pub(crate) fn array<const N: usize>(&mut self) -> Result<[u8; N], ArtifactError> {
        self.take(N)?.try_into().map_err(|_| ArtifactError::Truncated)
    }
}

#[cfg(feature = "persistence")]
pub(crate) fn restore_leaves<H: TreeHasher>(
    leaves: Leaves,
    addresses: BTreeMap<B256, Address>,
    slots: BTreeMap<Vec<u8>, U256>,
    root: B256,
    limits: DecodeLimits,
) -> Result<StateTree<H>, ArtifactError> {
    let mut headers = BTreeMap::new();
    let mut plain_slots: BTreeMap<Address, Vec<B256>> = BTreeMap::new();
    for (hash, address) in &addresses {
        if key::account_hash::<H>(*address) != *hash {
            return Err(ArtifactError::Invalid("invalid account preimage"));
        }
        let basic = leaves
            .get(key::header_key::<H>(*address, key::BASIC_DATA_LEAF_KEY).as_slice())
            .copied()
            .unwrap_or_default();
        let mut metadata = Reader(&basic[4..16]);
        let size = u32::from_be_bytes(metadata.array()?) as usize;
        let nonce = u64::from_be_bytes(metadata.array()?);
        let balance = U256::from_be_slice(&basic[16..]);
        let code_hash =
            leaves.get(key::header_key::<H>(*address, key::CODE_HASH_LEAF_KEY).as_slice());
        let delegation =
            leaves.get(key::header_key::<H>(*address, key::DELEGATION_LEAF_KEY).as_slice());
        let code = match (code_hash, delegation) {
            (Some(hash), None) if size == 0 && *hash == keccak256([]) => CodeRef::Empty,
            (Some(hash), None) if size > 0 => {
                if size > limits.max_code_size {
                    return Err(ArtifactError::ResourceLimit);
                }
                CodeRef::Contract(*hash, size)
            }
            (None, Some(value))
                if size == 23 &&
                    value[..3] == [0xef, 1, 0] &&
                    value[23..].iter().all(|byte| *byte == 0) =>
            {
                CodeRef::Delegation(Address::from_slice(&value[3..23]))
            }
            _ => return Err(ArtifactError::Invalid("invalid account code leaves")),
        };
        if matches!(code, CodeRef::Empty) && nonce == 0 && balance.is_zero() {
            return Err(ArtifactError::Invalid("empty anchor account"));
        }
        headers.insert(*hash, Header { nonce, balance, code });
        for slot in 0..key::HEADER_STORAGE_SLOTS {
            if leaves.contains_key(key::storage_key::<H>(*address, U256::from(slot)).as_slice()) {
                plain_slots.entry(*address).or_default().push(B256::from(U256::from(slot)));
            }
        }
    }
    for (key, slot) in slots {
        let hash = B256::from_slice(&key[1..33]);
        let address =
            *addresses.get(&hash).ok_or(ArtifactError::Invalid("storage without header"))?;
        if crate::key::storage_key::<H>(address, slot) != key {
            return Err(ArtifactError::Invalid("invalid storage preimage"));
        }
        plain_slots.entry(address).or_default().push(B256::from(slot));
    }
    let mut accounts: Vec<_> = addresses.values().copied().collect();
    accounts.sort_unstable_by_key(|address| keccak256(address));
    let mut preimages = Vec::new();
    for address in accounts {
        preimages.extend_from_slice(address.as_slice());
        let mut slots = plain_slots.remove(&address).unwrap_or_default();
        slots.sort_unstable_by_key(|slot| keccak256(slot));
        let count = u32::try_from(slots.len()).map_err(|_| ArtifactError::ResourceLimit)?;
        preimages.extend_from_slice(&count.to_be_bytes());
        for slot in slots {
            preimages.extend_from_slice(slot.as_slice());
        }
    }
    decode_state::<H>(&headers, &leaves, &preimages, root, limits)
}

//! Fixtures shared by the crate's tests: headers, an account trie and a scripted snap client.

use crate::{
    common::{DEFAULT_RESPONSE_BYTES, MAX_HASH},
    SnapGeneration, SnapPivotPolicy,
};
use alloy_consensus::Header;
use alloy_eip7928::{compute_block_access_list_hash, AccountChanges};
use alloy_eips::{eip7928::bal::Bal, BlockNumHash};
use alloy_primitives::{Bytes, B256, KECCAK256_EMPTY, U256};
use futures::future::{ready, Ready};
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    table::Table,
    tables,
    transaction::DbTx,
};
use reth_downloaders::snap::{AccountRangeDownloader, AccountRangeOutcome, VerifiedAccountRange};
use reth_eth_wire_types::{
    snap::{
        AccountData, AccountRangeMessage, BlockAccessListsMessage, ByteCodesMessage,
        GetAccountRangeMessage, GetBlockAccessListsMessage, GetByteCodesMessage,
        GetStorageRangesMessage, StorageData, StorageRangesMessage,
    },
    BlockAccessLists,
};
use reth_network_p2p::{
    download::DownloadClient,
    error::{PeerRequestResult, RequestError},
    priority::Priority,
    snap::client::{SnapClient, SnapResponse},
};
use reth_network_peers::{PeerId, WithPeerId};
use reth_primitives_traits::{Account, AlloyBlockHeader, Bytecode, SealedHeader, StorageEntry};
use reth_provider::{
    test_utils::{
        create_test_provider_factory, insert_headers, MockEthProvider, MockNodeTypesWithDB,
    },
    DatabaseProviderFactory, ProviderFactory, StaticFileProviderFactory, StaticFileSegment,
    StaticFileWriter,
};
use reth_storage_api::{DBProvider, MetadataWriter, StorageSettings, StorageSettingsCache};
use reth_tasks::Runtime;
use reth_trie_common::{
    proof::ProofRetainer,
    root::{state_root_unsorted, storage_root_unsorted},
    HashBuilder, Nibbles, TrieAccount, EMPTY_ROOT_HASH,
};
use std::{
    collections::VecDeque,
    ops::Range,
    sync::{Mutex, MutexGuard},
};

/// Small bounds keep header fixtures short without changing the policy's decisions.
pub(crate) fn policy() -> SnapPivotPolicy {
    SnapPivotPolicy::default().with_head_distance(1).with_advance_after(4).with_history(8)
}

/// A header with a state root distinctive to its number, and a commitment when one is given.
pub(crate) fn header(
    number: u64,
    parent_hash: B256,
    block_access_list_hash: Option<B256>,
) -> Header {
    Header {
        number,
        parent_hash,
        state_root: B256::repeat_byte(number as u8),
        block_access_list_hash,
        ..Default::default()
    }
}

/// Blocks `0..=3`, carrying a block access list commitment from `bal_from` onwards.
pub(crate) fn chain(bal_from: Option<u64>) -> Vec<Header> {
    let mut headers = Vec::new();
    let mut parent = B256::ZERO;
    for number in 0..=3 {
        let bal =
            bal_from.filter(|from| number >= *from).map(|_| B256::with_last_byte(number as u8));
        let header = header(number, parent, bal);
        parent = header.hash_slow();
        headers.push(header);
    }
    headers
}

pub(crate) fn provider_with(headers: impl IntoIterator<Item = Header>) -> MockEthProvider {
    let provider = MockEthProvider::default();
    provider.extend_headers(headers.into_iter().map(|header| (header.hash_slow(), header)));
    provider
}

/// A generation anchored to `block`, downloading against `state_root`.
pub(crate) fn generation(block: u64, state_root: B256) -> SnapGeneration {
    SnapGeneration::new(BlockNumHash::new(block, B256::repeat_byte(block as u8)), state_root)
}

/// Writes blocks `0..=3` hashed the way [`generation`] anchors them, so an attempt started from
/// one finds its pivot on the canonical chain.
pub(crate) fn insert_generation_headers(factory: &ProviderFactory<MockNodeTypesWithDB>) {
    let headers: Vec<_> = (0..=3u64)
        .map(|number| {
            let parent = number.checked_sub(1).map_or(B256::ZERO, |n| B256::repeat_byte(n as u8));
            SealedHeader::new(header(number, parent, None), B256::repeat_byte(number as u8))
        })
        .collect();
    insert_headers(factory, &headers);
}

/// A database using the hashed state layout snap writes into.
pub(crate) fn hashed_factory() -> ProviderFactory<MockNodeTypesWithDB> {
    let factory = create_test_provider_factory();
    let provider = factory.database_provider_rw().unwrap();
    provider.write_storage_settings(StorageSettings::v2()).unwrap();
    provider.commit().unwrap();
    // Writers consult the cache, not the table.
    factory.set_storage_settings_cache(StorageSettings::v2());
    factory
}

/// A hashed account key in the lowest part of the key space.
pub(crate) fn key(value: u64) -> B256 {
    B256::left_padding_from(&value.to_be_bytes())
}

/// An account without storage or code, distinguished by its nonce.
pub(crate) fn account(nonce: u64) -> TrieAccount {
    TrieAccount {
        nonce,
        balance: U256::from(1),
        storage_root: EMPTY_ROOT_HASH,
        code_hash: KECCAK256_EMPTY,
    }
}

/// Root of the account trie holding `accounts`.
pub(crate) fn state_root(accounts: &[(B256, TrieAccount)]) -> B256 {
    state_root_unsorted(accounts.iter().copied())
}

// Root of the account trie, and the proof nodes on the paths to `targets`.
fn root_and_proof(accounts: &[(B256, TrieAccount)], targets: &[B256]) -> (B256, Vec<Bytes>) {
    trie(accounts.iter().map(|(key, account)| (*key, alloy_rlp::encode(account))), targets)
}

/// Root of the storage trie holding `slots`.
pub(crate) fn storage_root_of(slots: &[(B256, U256)]) -> B256 {
    storage_root_unsorted(slots.iter().copied())
}

// Root of the storage trie, and the proof nodes on the paths to `targets`.
fn storage_trie(slots: &[(B256, U256)], targets: &[B256]) -> (B256, Vec<Bytes>) {
    trie(slots.iter().map(|(slot, value)| (*slot, alloy_rlp::encode(value))), targets)
}

// Root of the trie holding `leaves` in key order, and the proof nodes on the paths to `targets`.
fn trie(leaves: impl IntoIterator<Item = (B256, Vec<u8>)>, targets: &[B256]) -> (B256, Vec<Bytes>) {
    let targets = targets.iter().copied().map(Nibbles::unpack).collect();
    let mut builder = HashBuilder::default().with_proof_retainer(ProofRetainer::new(targets));
    for (key, leaf) in leaves {
        builder.add_leaf(Nibbles::unpack(key), &leaf);
    }
    let root = builder.root();
    let proof =
        builder.take_proof_nodes().into_nodes_sorted().into_iter().map(|(_, node)| node).collect();
    (root, proof)
}

/// A peer's answer serving `accounts[served]` out of the trie holding `accounts`, proven along
/// the paths to `proof_targets`. No targets means the whole trie is served without a proof.
pub(crate) fn account_range(
    request_id: u64,
    accounts: &[(B256, TrieAccount)],
    served: Range<usize>,
    proof_targets: &[B256],
) -> PeerRequestResult<SnapResponse> {
    // A complete trie needs no proof; the retainer would still emit the root node.
    let proof = if proof_targets.is_empty() {
        Vec::new()
    } else {
        root_and_proof(accounts, proof_targets).1
    };
    let message = AccountRangeMessage {
        request_id,
        accounts: accounts[served]
            .iter()
            .map(|(key, account)| AccountData::from_trie_account(*key, account))
            .collect(),
        proof,
    };
    Ok(WithPeerId::new(PeerId::random(), SnapResponse::AccountRange(message)))
}

/// Runs the same verification production uses on [`account_range`]'s answer to a request from
/// `origin` through the end of the key space.
pub(crate) fn verified_range(
    accounts: &[(B256, TrieAccount)],
    served: Range<usize>,
    origin: B256,
    proof_targets: &[B256],
) -> VerifiedAccountRange {
    let client = ScriptedSnapClient::new([account_range(1, accounts, served, proof_targets)]);
    let request = GetAccountRangeMessage {
        request_id: 1,
        root_hash: state_root(accounts),
        starting_hash: origin,
        limit_hash: MAX_HASH,
        response_bytes: DEFAULT_RESPONSE_BYTES,
    };
    let downloader = AccountRangeDownloader::new(client, request, Runtime::test()).unwrap();
    match futures::executor::block_on(downloader).unwrap() {
        AccountRangeOutcome::Verified(range) => range,
        AccountRangeOutcome::Unavailable { .. } => panic!("fixture serves the requested root"),
    }
}

/// A peer's answer serving `ranges`, the last of which belongs to the storage trie holding `last`
/// and is proven along the paths to `proof_targets`. No targets means no proof.
pub(crate) fn storage_ranges(
    request_id: u64,
    ranges: &[&[(B256, U256)]],
    last: &[(B256, U256)],
    proof_targets: &[B256],
) -> PeerRequestResult<SnapResponse> {
    let proof =
        if proof_targets.is_empty() { Vec::new() } else { storage_trie(last, proof_targets).1 };
    let slots = ranges
        .iter()
        .map(|range| {
            range.iter().map(|(slot, value)| StorageData::from_value(*slot, *value)).collect()
        })
        .collect();
    let message = StorageRangesMessage { request_id, slots, proof };
    Ok(WithPeerId::new(PeerId::random(), SnapResponse::StorageRanges(message)))
}

/// A peer's answer serving `codes`, in the order they were requested.
pub(crate) fn byte_codes(request_id: u64, codes: &[Bytes]) -> PeerRequestResult<SnapResponse> {
    let message = ByteCodesMessage { request_id, codes: codes.to_vec() };
    Ok(WithPeerId::new(PeerId::random(), SnapResponse::ByteCodes(message)))
}

/// A canonical chain from genesis through the blocks a catch-up applies, each of those carrying
/// the list its header commits to.
pub(crate) struct BalChain {
    /// Headers from genesis through the last block after the pivot.
    pub(crate) headers: Vec<SealedHeader<Header>>,
    // Block the downloaded state is anchored to.
    pivot: u64,
    // Encoded lists of the blocks after the pivot, as peers serve them.
    lists: Vec<Bytes>,
}

impl BalChain {
    /// A chain anchored at `pivot`, carrying one block per entry of `lists` after it.
    pub(crate) fn new(pivot: u64, lists: impl IntoIterator<Item = Vec<AccountChanges>>) -> Self {
        let (commitments, lists): (Vec<B256>, Vec<Bytes>) = lists
            .into_iter()
            .map(|changes| {
                (
                    compute_block_access_list_hash(&changes),
                    alloy_rlp::encode(Bal::from(changes)).into(),
                )
            })
            .unzip();
        let mut headers = Vec::new();
        let mut parent = B256::ZERO;
        for number in 0..=pivot {
            headers.push(SealedHeader::seal_slow(header(number, parent, None)));
            parent = headers[number as usize].hash();
        }
        for (index, commitment) in commitments.into_iter().enumerate() {
            let sealed =
                SealedHeader::seal_slow(header(pivot + index as u64 + 1, parent, Some(commitment)));
            parent = sealed.hash();
            headers.push(sealed);
        }
        Self { headers, pivot, lists }
    }

    /// Generation anchored to this chain's pivot, downloading against `state_root`.
    pub(crate) fn generation(&self, state_root: B256) -> SnapGeneration {
        SnapGeneration::new(self.block(0), state_root)
    }

    /// The chain's last block.
    pub(crate) fn tip(&self) -> BlockNumHash {
        self.block(self.lists.len())
    }

    /// Replaces the canonical tip with this chain's tip, retaining its ancestors.
    pub(crate) fn replace_tip(&self, factory: &ProviderFactory<MockNodeTypesWithDB>) {
        let static_files = factory.static_file_provider();
        let mut writer = static_files.latest_writer(StaticFileSegment::Headers).unwrap();
        writer.prune_headers(1).unwrap();
        writer.commit().unwrap();
        let tip = self.headers.last().unwrap();
        writer.append_header(tip.header(), &tip.hash()).unwrap();
        writer.commit().unwrap();
    }

    /// The block `nth` after the pivot, which is the pivot itself at zero.
    pub(crate) fn block(&self, nth: usize) -> BlockNumHash {
        let header = &self.headers[self.pivot as usize + nth];
        BlockNumHash::new(header.number(), header.hash())
    }

    /// A peer's answer serving the list of each block `served` names, holding none where it names
    /// no block.
    pub(crate) fn response(
        &self,
        request_id: u64,
        served: impl IntoIterator<Item = Option<usize>>,
    ) -> PeerRequestResult<SnapResponse> {
        let block_access_lists = served
            .into_iter()
            .map(|nth| nth.map(|nth| self.lists[nth - 1].clone()))
            .collect::<Vec<_>>();
        let message = BlockAccessListsMessage {
            request_id,
            block_access_lists: BlockAccessLists(block_access_lists),
        };
        Ok(WithPeerId::new(PeerId::random(), SnapResponse::BlockAccessLists(message)))
    }
}

/// Slots persisted for `account`, in key order.
pub(crate) fn stored_slots(provider: &impl DBProvider, account: B256) -> Vec<(B256, U256)> {
    let mut cursor = provider.tx_ref().cursor_dup_read::<tables::HashedStorages>().unwrap();
    cursor
        .walk_dup(Some(account), None)
        .unwrap()
        .map(|entry| {
            let (_, slot) = entry.unwrap();
            (slot.key, slot.value)
        })
        .collect()
}

/// Persisted snap state and all metadata, including attempt, coverage and catch-up progress.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SnapStateSnapshot {
    accounts: Vec<(B256, Account)>,
    storages: Vec<(B256, StorageEntry)>,
    bytecodes: Vec<(B256, Bytecode)>,
    metadata: Vec<(String, Vec<u8>)>,
}

impl SnapStateSnapshot {
    pub(crate) fn read(provider: &impl DBProvider) -> Self {
        Self {
            accounts: table_entries::<tables::HashedAccounts>(provider),
            storages: table_entries::<tables::HashedStorages>(provider),
            bytecodes: table_entries::<tables::Bytecodes>(provider),
            metadata: table_entries::<tables::Metadata>(provider),
        }
    }
}

fn table_entries<T: Table>(provider: &impl DBProvider) -> Vec<(T::Key, T::Value)> {
    provider.tx_ref().cursor_read::<T>().unwrap().walk(None).unwrap().map(Result::unwrap).collect()
}

/// Serves scripted answers in request order, recording what each request asked for.
pub(crate) struct ScriptedSnapClient {
    responses: Mutex<VecDeque<PeerRequestResult<SnapResponse>>>,
    origins: Mutex<Vec<B256>>,
    storage_requests: Mutex<Vec<(Vec<B256>, B256)>>,
    code_requests: Mutex<Vec<Vec<B256>>>,
    block_requests: Mutex<Vec<Vec<B256>>>,
    on_block_request: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl ScriptedSnapClient {
    pub(crate) fn new(
        responses: impl IntoIterator<Item = PeerRequestResult<SnapResponse>>,
    ) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            origins: Mutex::new(Vec::new()),
            storage_requests: Mutex::new(Vec::new()),
            code_requests: Mutex::new(Vec::new()),
            block_requests: Mutex::new(Vec::new()),
            on_block_request: Mutex::new(None),
        }
    }

    /// Runs `hook` after the next BAL request is recorded, before returning its response.
    pub(crate) fn on_block_request(self, hook: impl FnOnce() + Send + 'static) -> Self {
        *self.on_block_request.lock().unwrap() = Some(Box::new(hook));
        self
    }

    /// Origins of the account range requests sent so far.
    pub(crate) fn origins(&self) -> MutexGuard<'_, Vec<B256>> {
        self.origins.lock().unwrap()
    }

    /// Accounts and starting slot of the storage range requests sent so far.
    pub(crate) fn storage_requests(&self) -> MutexGuard<'_, Vec<(Vec<B256>, B256)>> {
        self.storage_requests.lock().unwrap()
    }

    /// Hashes of the bytecode requests sent so far.
    pub(crate) fn code_requests(&self) -> MutexGuard<'_, Vec<Vec<B256>>> {
        self.code_requests.lock().unwrap()
    }

    /// Block hashes of the block access list requests sent so far.
    pub(crate) fn block_requests(&self) -> MutexGuard<'_, Vec<Vec<B256>>> {
        self.block_requests.lock().unwrap()
    }

    fn next_response(&self) -> Ready<PeerRequestResult<SnapResponse>> {
        let response = self.responses.lock().unwrap().pop_front();
        ready(response.unwrap_or(Err(RequestError::ChannelClosed)))
    }
}

impl std::fmt::Debug for ScriptedSnapClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptedSnapClient").finish_non_exhaustive()
    }
}

impl DownloadClient for ScriptedSnapClient {
    fn report_bad_message(&self, _peer_id: PeerId) {}

    fn num_connected_peers(&self) -> usize {
        1
    }
}

impl SnapClient for ScriptedSnapClient {
    type Output = Ready<PeerRequestResult<SnapResponse>>;

    fn get_account_range_with_priority(
        &self,
        request: GetAccountRangeMessage,
        _priority: Priority,
    ) -> Self::Output {
        self.origins.lock().unwrap().push(request.starting_hash);
        self.next_response()
    }

    fn get_storage_ranges(&self, request: GetStorageRangesMessage) -> Self::Output {
        self.get_storage_ranges_with_priority(request, Priority::Normal)
    }

    fn get_storage_ranges_with_priority(
        &self,
        request: GetStorageRangesMessage,
        _priority: Priority,
    ) -> Self::Output {
        let from = request.starting_hash.unwrap_or(B256::ZERO);
        self.storage_requests.lock().unwrap().push((request.account_hashes, from));
        self.next_response()
    }

    fn get_byte_codes(&self, request: GetByteCodesMessage) -> Self::Output {
        self.get_byte_codes_with_priority(request, Priority::Normal)
    }

    fn get_byte_codes_with_priority(
        &self,
        request: GetByteCodesMessage,
        _priority: Priority,
    ) -> Self::Output {
        self.code_requests.lock().unwrap().push(request.hashes);
        self.next_response()
    }

    fn get_block_access_lists_with_priority(
        &self,
        request: GetBlockAccessListsMessage,
        _priority: Priority,
    ) -> Self::Output {
        self.block_requests.lock().unwrap().push(request.block_hashes);
        if let Some(hook) = self.on_block_request.lock().unwrap().take() {
            hook();
        }
        self.next_response()
    }
}

//! Fixtures shared by the crate's tests: headers, an account trie and a scripted snap client.

use crate::{SnapGeneration, SnapPivotPolicy};
use alloy_consensus::Header;
use alloy_eips::BlockNumHash;
use alloy_primitives::{Bytes, B256, KECCAK256_EMPTY, U256};
use futures::future::{ready, Ready};
use reth_downloaders::snap::{AccountRangeDownloader, AccountRangeOutcome, VerifiedAccountRange};
use reth_eth_wire_types::snap::{
    AccountData, AccountRangeMessage, GetAccountRangeMessage, GetBlockAccessListsMessage,
    GetByteCodesMessage, GetStorageRangesMessage,
};
use reth_network_p2p::{
    download::DownloadClient,
    error::{PeerRequestResult, RequestError},
    priority::Priority,
    snap::client::{SnapClient, SnapResponse},
};
use reth_network_peers::{PeerId, WithPeerId};
use reth_provider::{
    test_utils::{create_test_provider_factory, MockEthProvider, MockNodeTypesWithDB},
    DatabaseProviderFactory, ProviderFactory,
};
use reth_storage_api::{DBProvider, MetadataWriter, StorageSettings};
use reth_tasks::Runtime;
use reth_trie_common::{proof::ProofRetainer, HashBuilder, Nibbles, TrieAccount, EMPTY_ROOT_HASH};
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

/// A database using the hashed state layout snap writes into.
pub(crate) fn hashed_factory() -> ProviderFactory<MockNodeTypesWithDB> {
    let factory = create_test_provider_factory();
    let provider = factory.database_provider_rw().unwrap();
    provider.write_storage_settings(StorageSettings::v2()).unwrap();
    provider.commit().unwrap();
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
    root_and_proof(accounts, &[]).0
}

// Root of the trie, and the proof nodes on the paths to `targets`.
fn root_and_proof(accounts: &[(B256, TrieAccount)], targets: &[B256]) -> (B256, Vec<Bytes>) {
    let targets = targets.iter().copied().map(Nibbles::unpack).collect();
    let mut builder = HashBuilder::default().with_proof_retainer(ProofRetainer::new(targets));
    for (key, account) in accounts {
        builder.add_leaf(Nibbles::unpack(*key), &alloy_rlp::encode(account));
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
        limit_hash: B256::new([0xff; B256::len_bytes()]),
        response_bytes: 512 * 1024,
    };
    let downloader = AccountRangeDownloader::new(client, request, Runtime::test()).unwrap();
    match futures::executor::block_on(downloader).unwrap() {
        AccountRangeOutcome::Verified(range) => range,
        AccountRangeOutcome::Unavailable { .. } => panic!("fixture serves the requested root"),
    }
}

/// Serves scripted answers in request order, recording what each request asked for.
pub(crate) struct ScriptedSnapClient {
    responses: Mutex<VecDeque<PeerRequestResult<SnapResponse>>>,
    origins: Mutex<Vec<B256>>,
}

impl ScriptedSnapClient {
    pub(crate) fn new(
        responses: impl IntoIterator<Item = PeerRequestResult<SnapResponse>>,
    ) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            origins: Mutex::new(Vec::new()),
        }
    }

    /// Origins of the account range requests sent so far.
    pub(crate) fn origins(&self) -> MutexGuard<'_, Vec<B256>> {
        self.origins.lock().unwrap()
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
        let response = self.responses.lock().unwrap().pop_front();
        ready(response.unwrap_or(Err(RequestError::ChannelClosed)))
    }

    fn get_storage_ranges(&self, _request: GetStorageRangesMessage) -> Self::Output {
        unsupported()
    }

    fn get_storage_ranges_with_priority(
        &self,
        _request: GetStorageRangesMessage,
        _priority: Priority,
    ) -> Self::Output {
        unsupported()
    }

    fn get_byte_codes(&self, _request: GetByteCodesMessage) -> Self::Output {
        unsupported()
    }

    fn get_byte_codes_with_priority(
        &self,
        _request: GetByteCodesMessage,
        _priority: Priority,
    ) -> Self::Output {
        unsupported()
    }

    fn get_block_access_lists_with_priority(
        &self,
        _request: GetBlockAccessListsMessage,
        _priority: Priority,
    ) -> Self::Output {
        unsupported()
    }
}

fn unsupported() -> Ready<PeerRequestResult<SnapResponse>> {
    ready(Err(RequestError::UnsupportedCapability))
}

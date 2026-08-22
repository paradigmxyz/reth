//! Recovers a Snap generation whose pivot was orphaned by a reorg.
//!
//! EIP-8189 defines the recovery: find the common ancestor `W` of the orphaned and canonical
//! chains, collect the orphaned blocks' block access lists, delete the entries they mutated, and
//! re-fetch those entries before continuing from `W`.
//!
//! Two deliberate narrowings keep the work bounded. Entries the canonical chain also mutates are
//! re-fetched rather than skipped, because identifying them would mean downloading the canonical
//! block access lists that catch-up applies afterwards anyway. Entries at or beyond the account
//! cursor are ignored, because nothing was downloaded there yet.

use crate::{
    download::{push_peer, request_options},
    error::db_error,
    SnapGeneration, SnapStateStore, SnapSyncError, StateDownloader,
};
use alloy_eip7928::bal::DecodedBal;
use alloy_primitives::{keccak256, map::B256Set, Sealable, B256};
use reth_downloaders::snap::{BlockAccessListDownloader, BlockAccessListOutcome};
use reth_eth_wire_types::snap::GetBlockAccessListsMessage;
use reth_network_p2p::{
    error::RequestError,
    headers::client::{HeadersClient, HeadersRequest},
    snap::client::SnapClient,
};
use reth_primitives_traits::{AlloyBlockHeader, BlockHeader, SealedHeader};
use reth_provider::DatabaseProviderFactory;
use reth_storage_api::{
    DBProvider, HeaderProvider, StageCheckpointReader, StageCheckpointWriter, StateWriter,
    StorageSettingsCache,
};
use reth_tasks::Runtime;
use reth_trie_common::{bal::BalAccountState, HashedPostState, HashedStorage};
use tracing::debug;

// EIP-8189 recommends a 2 MiB BAL response to limit memory and round-trip overhead.
const BAL_RESPONSE_BYTES: u64 = 2 * 1024 * 1024;
// Twenty-eight average 60M-gas BALs fit below the recommended response size.
const BAL_BLOCKS_PER_REQUEST: usize = 28;
// One batch covers the depth of any reorg a pivot is expected to survive.
const FORK_HEADERS_PER_REQUEST: u64 = 64;
// A fork deeper than this costs more to reconcile than a fresh generation.
const DEFAULT_MAX_FORK_DEPTH: u64 = 512;
// Re-fetching more entries than this is slower than downloading the state again.
const DEFAULT_MAX_RESTORED_ACCOUNTS: usize = 65_536;
// Peers that answer with gaps or broken chains are retried a bounded number of times.
const DEFAULT_MAX_PEER_ATTEMPTS: usize = 8;

/// Reconciles a generation with the canonical chain after its pivot was orphaned.
#[derive(Debug)]
pub struct PivotReorgRecovery<'a, C, H, F> {
    // Serves the orphaned block access lists and the re-fetched state.
    client: &'a C,
    // Walks the orphaned chain back to the common ancestor.
    headers: &'a H,
    // Header reads and state writes observe the same provider factory.
    factory: &'a F,
    // The recovered anchor and its restored state commit together.
    store: SnapStateStore<'a, F>,
    // Decoding and proof verification stay off the async worker.
    runtime: Runtime,
    // Bounds that decide when restarting is cheaper than recovering.
    limits: ReorgLimits,
    // Request IDs remain unique for this recovery attempt.
    request_id: u64,
}

impl<'a, C, H, F> PivotReorgRecovery<'a, C, H, F> {
    /// Creates a recovery without starting network or database work.
    pub const fn new(client: &'a C, headers: &'a H, factory: &'a F, runtime: Runtime) -> Self {
        Self {
            client,
            headers,
            factory,
            store: SnapStateStore::new(factory),
            runtime,
            limits: ReorgLimits::new(),
            request_id: 0,
        }
    }

    /// Sets the bounds beyond which a generation is abandoned instead of recovered.
    pub const fn with_limits(mut self, limits: ReorgLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Re-anchors `generation` to the common ancestor, restoring the entries the orphaned chain
    /// mutated.
    ///
    /// [`ReorgRecoveryOutcome::Unrecoverable`] means the caller must restart the generation: the
    /// fork is too deep, changed too much, or peers no longer serve what recovery needs.
    pub async fn run(
        &mut self,
        generation: SnapGeneration,
    ) -> Result<ReorgRecoveryOutcome, SnapSyncError>
    where
        C: SnapClient,
        H: HeadersClient<Header: BlockHeader + Sealable>,
        F: DatabaseProviderFactory<Provider: HeaderProvider>,
        F::ProviderRW: DBProvider
            + HeaderProvider
            + StageCheckpointReader
            + StageCheckpointWriter
            + StateWriter
            + StorageSettingsCache,
    {
        let Some(fork) = self.orphaned_fork(generation).await? else {
            return Ok(ReorgRecoveryOutcome::Unrecoverable)
        };
        let Some(mutated) = self.mutated_accounts(&fork.orphans).await? else {
            return Ok(ReorgRecoveryOutcome::Unrecoverable)
        };

        // Only the downloaded prefix holds state a reorg can have invalidated.
        let mut restored = mutated
            .into_iter()
            .filter(|hashed_address| *hashed_address < generation.next_account)
            .collect::<Vec<_>>();
        if restored.len() > self.limits.max_restored_accounts {
            debug!(
                target: "snap::reorg",
                accounts = restored.len(),
                "Orphaned fork mutated more state than recovery restores"
            );
            return Ok(ReorgRecoveryOutcome::Unrecoverable)
        }
        restored.sort_unstable();

        let Some(downloaded) =
            StateDownloader::new(self.client, self.factory, self.runtime.clone())
                .download_accounts(fork.state_root, &restored)
                .await?
        else {
            return Ok(ReorgRecoveryOutcome::Unrecoverable)
        };

        let generation = self.store.commit_reanchor(
            generation,
            fork.ancestor_block,
            fork.ancestor_hash,
            restored_state(downloaded.state, &restored),
            downloaded.bytecodes,
        )?;
        Ok(ReorgRecoveryOutcome::Recovered { generation })
    }

    // Walks the orphaned chain back until one of its blocks is canonical locally.
    async fn orphaned_fork(
        &self,
        generation: SnapGeneration,
    ) -> Result<Option<OrphanedFork<H::Header>>, SnapSyncError>
    where
        H: HeadersClient<Header: BlockHeader + Sealable>,
        F: DatabaseProviderFactory<Provider: HeaderProvider>,
    {
        let mut orphans = Vec::new();
        let mut parent = generation.target_hash;
        let mut attempts = 0;

        // Requesting only what the depth bound allows keeps a long response from exceeding it.
        while let Some(remaining) =
            self.limits.max_fork_depth.checked_sub(orphans.len() as u64).filter(|it| *it > 0)
        {
            let response = self
                .headers
                .get_headers(HeadersRequest::falling(
                    parent.into(),
                    remaining.min(FORK_HEADERS_PER_REQUEST),
                ))
                .await;
            let headers = match response {
                Ok(headers) => headers,
                Err(RequestError::UnsupportedCapability) => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            let (peer_id, headers) = headers.split();
            if headers.is_empty() {
                attempts += 1;
                if attempts >= self.limits.max_peer_attempts {
                    return Ok(None)
                }
                continue
            }

            for header in headers {
                if orphans.len() as u64 >= self.limits.max_fork_depth {
                    break
                }
                let header = SealedHeader::seal_slow(header);
                // A response that leaves the requested chain cannot be authenticated further.
                if header.hash() != parent {
                    self.headers.report_bad_message(peer_id);
                    attempts += 1;
                    break
                }
                if let Some(fork) = self.ancestor(&header, &mut orphans)? {
                    return Ok(Some(fork))
                }
                parent = header.parent_hash();
                orphans.push(header);
            }
            if attempts >= self.limits.max_peer_attempts {
                return Ok(None)
            }
        }

        debug!(target: "snap::reorg", depth = orphans.len(), "Orphaned fork is deeper than recovery walks");
        Ok(None)
    }

    // A header that is canonical locally is the common ancestor of both chains.
    fn ancestor(
        &self,
        header: &SealedHeader<H::Header>,
        orphans: &mut Vec<SealedHeader<H::Header>>,
    ) -> Result<Option<OrphanedFork<H::Header>>, SnapSyncError>
    where
        H: HeadersClient<Header: BlockHeader + Sealable>,
        F: DatabaseProviderFactory<Provider: HeaderProvider>,
    {
        let provider = self.factory.database_provider_ro().map_err(db_error)?;
        let Some(canonical) = provider.sealed_header(header.number()).map_err(db_error)? else {
            return Ok(None)
        };
        if canonical.hash() != header.hash() {
            return Ok(None)
        }
        if orphans.is_empty() {
            debug!(target: "snap::reorg", number = header.number(), "Pivot is still canonical");
            return Ok(None)
        }
        // The walk collected the orphans from the pivot down; catch-up needs them in block order.
        orphans.reverse();
        Ok(Some(OrphanedFork {
            ancestor_block: header.number(),
            ancestor_hash: header.hash(),
            state_root: canonical.state_root(),
            orphans: core::mem::take(orphans),
        }))
    }

    // Collects every account the orphaned chain mutated, from authenticated block access lists.
    async fn mutated_accounts(
        &mut self,
        orphans: &[SealedHeader<H::Header>],
    ) -> Result<Option<B256Set>, SnapSyncError>
    where
        C: SnapClient,
        H: HeadersClient<Header: BlockHeader + Sealable>,
    {
        let mut mutated = B256Set::default();
        let mut pending = orphans;
        let mut excluded = Vec::new();

        while !pending.is_empty() {
            if excluded.len() >= self.limits.max_peer_attempts {
                debug!(target: "snap::reorg", "No peer serves the orphaned block access lists");
                return Ok(None)
            }
            let batch = &pending[..pending.len().min(BAL_BLOCKS_PER_REQUEST)];
            let request = GetBlockAccessListsMessage {
                request_id: self.next_request_id()?,
                block_hashes: batch.iter().map(|header| header.hash()).collect(),
                response_bytes: BAL_RESPONSE_BYTES,
            };
            let downloader = BlockAccessListDownloader::new_with_options(
                self.client,
                request,
                batch,
                self.runtime.clone(),
                request_options(&excluded),
            )
            .map_err(|error| SnapSyncError::InvalidRequest(error.to_string()))?;

            match downloader.await {
                Ok(BlockAccessListOutcome::Unavailable { peer_id }) => {
                    push_peer(&mut excluded, peer_id)
                }
                Err(RequestError::UnsupportedCapability) => return Ok(None),
                Err(error) => return Err(error.into()),
                Ok(BlockAccessListOutcome::Verified(verified)) => {
                    let mut applied = 0;
                    for (_, block_access_list) in verified.block_access_lists {
                        let Some(block_access_list) = block_access_list else { break };
                        collect_mutated(&block_access_list, &mut mutated);
                        applied += 1;
                    }
                    pending = &pending[applied..];
                    if applied < batch.len() {
                        // A gap makes this peer useless for the blocks still missing.
                        push_peer(&mut excluded, verified.peer_id);
                    } else {
                        excluded.clear();
                    }
                }
            }
        }
        Ok(Some(mutated))
    }

    // Failing on wrap prevents a stale response from matching a new logical request.
    fn next_request_id(&mut self) -> Result<u64, SnapSyncError> {
        self.request_id = self.request_id.checked_add(1).ok_or_else(|| {
            SnapSyncError::InvalidRequest("snap request id space exhausted".to_string())
        })?;
        Ok(self.request_id)
    }
}

/// Terminal result of one reorg recovery attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReorgRecoveryOutcome {
    /// The generation was re-anchored to the common ancestor with its state restored.
    Recovered {
        /// Generation anchored at the common ancestor.
        generation: SnapGeneration,
    },
    /// The generation must be restarted instead of recovered.
    Unrecoverable,
}

/// Bounds beyond which restarting a generation costs less than recovering it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReorgLimits {
    /// Orphaned blocks the ancestor walk visits before giving up.
    pub max_fork_depth: u64,
    /// Accounts re-fetched at the common ancestor before giving up.
    pub max_restored_accounts: usize,
    /// Peers tried for a header or block access list before giving up.
    pub max_peer_attempts: usize,
}

impl Default for ReorgLimits {
    fn default() -> Self {
        Self::new()
    }
}

impl ReorgLimits {
    /// Creates the default limits in a const context.
    pub const fn new() -> Self {
        Self {
            max_fork_depth: DEFAULT_MAX_FORK_DEPTH,
            max_restored_accounts: DEFAULT_MAX_RESTORED_ACCOUNTS,
            max_peer_attempts: DEFAULT_MAX_PEER_ATTEMPTS,
        }
    }
}

// The orphaned chain above the block both chains still share.
#[derive(Debug)]
struct OrphanedFork<H> {
    // Highest block shared by the orphaned and canonical chains.
    ancestor_block: u64,
    // Canonical hash at the ancestor.
    ancestor_hash: B256,
    // Canonical state root at the ancestor.
    state_root: B256,
    // Orphaned headers in block order.
    orphans: Vec<SealedHeader<H>>,
}

// Accounts whose fields or storage a block access list changed, rather than only accessed.
fn collect_mutated(block_access_list: &DecodedBal, mutated: &mut B256Set) {
    for changes in block_access_list.as_bal() {
        if !BalAccountState::from_changes(changes).is_empty() || !changes.storage_changes.is_empty()
        {
            mutated.insert(keccak256(changes.address));
        }
    }
}

// Entries missing from the ancestor's state are deleted, and every restored account has its
// storage replaced so slots written by the orphaned chain cannot survive.
fn restored_state(mut state: HashedPostState, restored: &[B256]) -> HashedPostState {
    for hashed_address in restored {
        state.accounts.entry(*hashed_address).or_insert(None);
        state.storages.entry(*hashed_address).or_insert_with(|| HashedStorage::new(true));
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AccountRangeProgress, SnapPhase};
    use alloy_consensus::Header;
    use alloy_eip7928::{bal::Bal, AccountChanges, BalanceChange, BlockAccessIndex};
    use alloy_primitives::{Address, Bytes, KECCAK256_EMPTY, U256};
    use reth_db_api::{tables, transaction::DbTx};
    use reth_downloaders::snap::test_utils::TestSnapClient;
    use reth_eth_wire_types::{
        snap::{AccountData, AccountRangeMessage, BlockAccessListsMessage},
        BlockAccessLists,
    };
    use reth_ethereum_primitives::BlockBody;
    use reth_network_p2p::{
        error::PeerRequestResult, snap::client::SnapResponse, test_utils::TestFullBlockClient,
    };
    use reth_network_peers::{PeerId, WithPeerId};
    use reth_primitives_traits::Account;
    use reth_provider::{
        test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
        DatabaseProviderFactory, ProviderFactory, StaticFileProviderFactory, StaticFileWriter,
    };
    use reth_static_file_types::StaticFileSegment;
    use reth_storage_api::StorageSettings;
    use reth_trie_common::{
        proof::ProofRetainer, HashBuilder, Nibbles, TrieAccount, EMPTY_ROOT_HASH,
    };

    // Three accounts ordered by hashed address: restored, deleted, and beyond the cursor.
    struct Accounts {
        deleted: (Address, B256),
        restored: (Address, B256),
        beyond_cursor: (Address, B256),
    }

    fn accounts() -> Accounts {
        let mut hashed = (1u8..=3)
            .map(|byte| {
                let address = Address::repeat_byte(byte);
                (address, keccak256(address))
            })
            .collect::<Vec<_>>();
        hashed.sort_unstable_by_key(|(_, hash)| *hash);
        Accounts { deleted: hashed[0], restored: hashed[1], beyond_cursor: hashed[2] }
    }

    fn ancestor_account() -> TrieAccount {
        TrieAccount {
            nonce: 9,
            balance: U256::from(9),
            storage_root: EMPTY_ROOT_HASH,
            code_hash: KECCAK256_EMPTY,
        }
    }

    fn root_and_proof(
        accounts: &[(B256, TrieAccount)],
        targets: &[B256],
    ) -> (B256, Vec<alloy_primitives::Bytes>) {
        let targets = targets.iter().copied().map(Nibbles::unpack).collect();
        let mut builder = HashBuilder::default().with_proof_retainer(ProofRetainer::new(targets));
        for (hash, account) in accounts {
            builder.add_leaf(Nibbles::unpack(*hash), &alloy_rlp::encode(account));
        }
        let root = builder.root();
        let proof = builder.take_proof_nodes().into_nodes_sorted().into_iter().map(|(_, n)| n);
        (root, proof.collect())
    }

    // Writes the canonical chain locally and returns its headers.
    fn canonical(
        factory: &ProviderFactory<MockNodeTypesWithDB>,
        ancestor_root: B256,
    ) -> Vec<Header> {
        let genesis = Header::default();
        let ancestor = Header {
            number: 1,
            parent_hash: genesis.hash_slow(),
            state_root: ancestor_root,
            block_access_list_hash: Some(B256::repeat_byte(0xaa)),
            ..Default::default()
        };
        let static_files = factory.static_file_provider();
        let mut writer = static_files.latest_writer(StaticFileSegment::Headers).unwrap();
        for header in [&genesis, &ancestor] {
            writer.append_header(header, &header.hash_slow()).unwrap();
        }
        writer.commit().unwrap();
        drop(writer);
        drop(static_files);
        vec![genesis, ancestor]
    }

    fn orphan(parent: &Header, block_access_list_hash: B256) -> Header {
        Header {
            number: parent.number + 1,
            parent_hash: parent.hash_slow(),
            state_root: B256::repeat_byte(0xee),
            block_access_list_hash: Some(block_access_list_hash),
            ..Default::default()
        }
    }

    fn block_access_list(addresses: impl IntoIterator<Item = Address>) -> (DecodedBal, Bytes) {
        let index = BlockAccessIndex::new(1);
        let mut changes = addresses
            .into_iter()
            .map(|address| {
                AccountChanges::new(address)
                    .with_balance_change(BalanceChange::new(index, U256::from(1)))
            })
            .collect::<Vec<_>>();
        changes.sort_unstable_by_key(|changes| changes.address);
        let raw = Bytes::from(alloy_rlp::encode(Bal::new(changes)));
        (DecodedBal::from_rlp_bytes(raw.clone()).unwrap(), raw)
    }

    fn bal_response(
        request_id: u64,
        entries: Vec<Option<Bytes>>,
    ) -> PeerRequestResult<SnapResponse> {
        Ok(WithPeerId::new(
            PeerId::random(),
            SnapResponse::BlockAccessLists(BlockAccessListsMessage {
                request_id,
                block_access_lists: BlockAccessLists(entries),
            }),
        ))
    }

    fn range_response(
        request_id: u64,
        accounts: &[(B256, TrieAccount)],
        proof: Vec<alloy_primitives::Bytes>,
    ) -> PeerRequestResult<SnapResponse> {
        Ok(WithPeerId::new(
            PeerId::random(),
            SnapResponse::AccountRange(AccountRangeMessage {
                request_id,
                accounts: accounts
                    .iter()
                    .map(|(hash, account)| AccountData::from_trie_account(*hash, account))
                    .collect(),
                proof,
            }),
        ))
    }

    #[tokio::test]
    async fn restores_orphaned_mutations_and_reanchors_the_generation() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());
        let Accounts { deleted, restored, beyond_cursor } = accounts();
        let ancestor_state = [(restored.1, ancestor_account())];
        let (ancestor_root, restored_proof) = root_and_proof(&ancestor_state, &[restored.1]);
        let (_, deleted_proof) = root_and_proof(&ancestor_state, &[deleted.1, restored.1]);
        let headers = canonical(&factory, ancestor_root);
        let (bal, raw_bal) = block_access_list([deleted.0, restored.0, beyond_cursor.0]);
        let orphan = orphan(&headers[1], bal.hash());

        let header_client = TestFullBlockClient::default();
        for header in headers.iter().chain([&orphan]) {
            header_client.insert(SealedHeader::seal_slow(header.clone()), BlockBody::default());
        }

        let store = SnapStateStore::new(&factory);
        let generation = SnapGeneration::new(orphan.number, orphan.hash_slow(), orphan.state_root);
        store.begin_generation(generation).unwrap();
        let generation = store
            .commit_account_range(
                generation,
                HashedPostState::default().with_accounts([
                    (deleted.1, Some(Account { nonce: 1, ..Default::default() })),
                    (restored.1, Some(Account { nonce: 2, ..Default::default() })),
                ]),
                Vec::new(),
                AccountRangeProgress::More { next_account: beyond_cursor.1 },
            )
            .unwrap();

        let client = TestSnapClient::new([
            bal_response(1, vec![Some(raw_bal)]),
            // The boundary account proves nothing exists at the deleted account's hash.
            range_response(1, &ancestor_state, deleted_proof),
            range_response(2, &ancestor_state, restored_proof),
        ]);

        let outcome = PivotReorgRecovery::new(&client, &header_client, &factory, Runtime::test())
            .run(generation)
            .await
            .unwrap();

        let ReorgRecoveryOutcome::Recovered { generation } = outcome else {
            panic!("recovered generation")
        };
        assert_eq!(generation.target_block, headers[1].number);
        assert_eq!(generation.target_hash, headers[1].hash_slow());
        assert_eq!(generation.state_root, ancestor_root);
        assert_eq!(generation.next_block, headers[1].number + 1);
        assert_eq!(generation.next_account, beyond_cursor.1);
        assert_eq!(generation.phase, SnapPhase::Accounts);
        assert_eq!(store.interrupted_generation().unwrap(), Some(generation));

        let provider = factory.database_provider_ro().unwrap();
        assert_eq!(provider.tx_ref().get::<tables::HashedAccounts>(deleted.1).unwrap(), None);
        assert_eq!(
            provider.tx_ref().get::<tables::HashedAccounts>(restored.1).unwrap(),
            Some(Account::from(ancestor_account()))
        );
        // The account beyond the cursor was never downloaded, so it is never re-fetched.
        assert_eq!(provider.tx_ref().get::<tables::HashedAccounts>(beyond_cursor.1).unwrap(), None);
    }

    #[tokio::test]
    async fn fork_deeper_than_the_limit_is_unrecoverable() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());
        let headers = canonical(&factory, B256::repeat_byte(0xbb));
        let (bal, _) = block_access_list([Address::repeat_byte(1)]);
        let first = orphan(&headers[1], bal.hash());
        let second = orphan(&first, bal.hash());
        let header_client = TestFullBlockClient::default();
        for header in headers.iter().chain([&first, &second]) {
            header_client.insert(SealedHeader::seal_slow(header.clone()), BlockBody::default());
        }
        let store = SnapStateStore::new(&factory);
        let generation = SnapGeneration::new(second.number, second.hash_slow(), second.state_root);
        store.begin_generation(generation).unwrap();
        let client = TestSnapClient::new(std::iter::empty());

        let outcome = PivotReorgRecovery::new(&client, &header_client, &factory, Runtime::test())
            .with_limits(ReorgLimits { max_fork_depth: 1, ..ReorgLimits::new() })
            .run(generation)
            .await
            .unwrap();

        assert_eq!(outcome, ReorgRecoveryOutcome::Unrecoverable);
        assert_eq!(store.interrupted_generation().unwrap(), Some(generation));
        assert!(client.priorities().is_empty());
    }

    #[tokio::test]
    async fn unserved_orphan_block_access_lists_are_unrecoverable() {
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(StorageSettings::v2());
        let headers = canonical(&factory, B256::repeat_byte(0xbb));
        let (bal, _) = block_access_list([Address::repeat_byte(1)]);
        let orphan = orphan(&headers[1], bal.hash());
        let header_client = TestFullBlockClient::default();
        for header in headers.iter().chain([&orphan]) {
            header_client.insert(SealedHeader::seal_slow(header.clone()), BlockBody::default());
        }
        let store = SnapStateStore::new(&factory);
        let generation = SnapGeneration::new(orphan.number, orphan.hash_slow(), orphan.state_root);
        store.begin_generation(generation).unwrap();
        let client = TestSnapClient::new(std::iter::empty());

        let outcome = PivotReorgRecovery::new(&client, &header_client, &factory, Runtime::test())
            .run(generation)
            .await
            .unwrap();

        assert_eq!(outcome, ReorgRecoveryOutcome::Unrecoverable);
        assert_eq!(store.interrupted_generation().unwrap(), Some(generation));
    }
}

//! Requests the lists of the canonical blocks between the applied one and the catch-up target.
//!
//! Headers are read again each request, so a reorged anchor is caught before peers are asked.

use crate::{
    common::DownloadContext, CatchUpProgress, SnapAttemptStore, SnapCatchUpStore, SnapSyncError,
    SnapWrite,
};
use alloy_eips::BlockNumHash;
use reth_db_api::transaction::DbTxMut;
use reth_downloaders::snap::{BlockAccessListDownloader, BlockAccessListOutcome};
use reth_eth_wire_types::snap::GetBlockAccessListsMessage;
use reth_network_p2p::snap::client::SnapClient;
use reth_network_peers::PeerId;
use reth_primitives_traits::{AlloyBlockHeader, SealedHeader};
use reth_storage_api::{
    BlockHashReader, DBProvider, DatabaseProviderFactory, HeaderProvider, MetadataProvider,
    MetadataWriter, StateWriter,
};
use reth_tasks::Runtime;
use std::fmt;

/// Default soft response limit for block access list requests, as EIP-8189 recommends.
pub const DEFAULT_BAL_RESPONSE_BYTES: u64 = 2 * 1024 * 1024;

/// Default number of blocks asked for per request, chosen so that many average 60M gas lists
/// still fit under [`DEFAULT_BAL_RESPONSE_BYTES`].
pub const DEFAULT_CATCH_UP_BLOCKS: u64 = 28;

/// Applies the lists of the canonical blocks after the applied one, one request at a time.
pub struct BlockAccessListCatchUp<C, F> {
    context: DownloadContext<C, F>,
    // Blocks asked for per request.
    max_blocks: u64,
}

impl<C, F> BlockAccessListCatchUp<C, F> {
    /// Creates a catch-up that requests the lists the applied state does not carry yet.
    pub const fn new(client: C, factory: F, runtime: Runtime) -> Self {
        let mut context = DownloadContext::new(client, factory, runtime);
        context.set_response_bytes(DEFAULT_BAL_RESPONSE_BYTES);
        Self { context, max_blocks: DEFAULT_CATCH_UP_BLOCKS }
    }

    /// Returns this catch-up asking peers for at most `response_bytes` per response.
    pub const fn with_response_bytes(mut self, response_bytes: u64) -> Self {
        self.context.set_response_bytes(response_bytes);
        self
    }

    /// Returns this catch-up asking for at most `max_blocks` blocks per request, at least one.
    pub const fn with_max_blocks(mut self, max_blocks: u64) -> Self {
        self.max_blocks = if max_blocks == 0 { 1 } else { max_blocks };
        self
    }
}

impl<C, F> BlockAccessListCatchUp<C, F>
where
    C: SnapClient + Clone + Unpin,
    F: DatabaseProviderFactory + Clone + 'static,
    F::Provider: HeaderProvider + MetadataProvider + BlockHashReader,
    F::ProviderRW:
        BlockHashReader + MetadataProvider + MetadataWriter + StateWriter + DBProvider<Tx: DbTxMut>,
{
    /// Requests the lists of the canonical blocks between the applied one and `target`, and
    /// commits those continuing it, in order.
    ///
    /// A list a peer leaves out ends the run this call commits, leaving it and every block after
    /// it to the next one.
    pub async fn next(
        &mut self,
        write: SnapWrite,
        target: u64,
    ) -> Result<CatchUpStep, SnapSyncError> {
        let max_blocks = self.max_blocks;
        let headers = self
            .context
            .read(move |provider| pending_headers(provider, write, target, max_blocks))
            .await?;
        if headers.is_empty() {
            return Ok(CatchUpStep::Complete)
        }

        let request = GetBlockAccessListsMessage {
            request_id: self.context.next_request_id(),
            block_hashes: headers.iter().map(SealedHeader::hash).collect(),
            response_bytes: self.context.response_bytes(),
        };
        let downloader = BlockAccessListDownloader::new(
            self.context.client().clone(),
            request,
            &headers,
            self.context.runtime().clone(),
        )?;
        let verified = match downloader.await? {
            BlockAccessListOutcome::Verified(verified) => verified,
            BlockAccessListOutcome::Unavailable { peer_id } => {
                return Ok(CatchUpStep::Unavailable { peer_id })
            }
        };

        let peer_id = verified.peer_id();
        // A list applies only once every earlier block's has, so the run stops at the first block
        // the peer left out.
        let applied = verified
            .into_block_access_lists()
            .into_iter()
            .zip(&headers)
            .map_while(|((_, list), header)| {
                let block = BlockNumHash::new(header.number(), header.hash());
                list.map(|list| (block, header.parent_hash(), list))
            })
            .collect::<Vec<_>>();
        if applied.is_empty() {
            return Ok(CatchUpStep::Unavailable { peer_id })
        }

        let blocks = applied.len();
        let progress = self
            .context
            .commit(move |provider| {
                let mut progress = None;
                for (block, parent, list) in applied {
                    progress = Some(provider.commit_block_access_list(
                        write,
                        block,
                        parent,
                        list.as_bal(),
                    )?);
                }
                Ok(progress.expect("the applied run is not empty"))
            })
            .await?;
        Ok(CatchUpStep::Applied { progress, blocks })
    }
}

impl<C, F> fmt::Debug for BlockAccessListCatchUp<C, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlockAccessListCatchUp")
            .field("context", &self.context)
            .field("max_blocks", &self.max_blocks)
            .finish()
    }
}

/// What one request of a [`BlockAccessListCatchUp`] produced.
#[derive(Debug)]
pub enum CatchUpStep {
    /// Lists were committed, carrying the downloaded state this far.
    Applied {
        /// How far the state is carried now.
        progress: CatchUpProgress,
        /// Blocks this request carried it past.
        blocks: usize,
    },
    /// The peer held no list for the next block, so the state stays where it is.
    Unavailable {
        /// Peer that answered without it, so the retry can go elsewhere.
        peer_id: PeerId,
    },
    /// Every block through the target is applied.
    Complete,
}

// The canonical headers continuing the applied block, at most `max_blocks` of them and none past
// `target` or the pivot.
//
// Both anchors must remain canonical: downloaded ranges can contain changes from the pivot
// even when catch-up has only reached an earlier block.
fn pending_headers<P: HeaderProvider + MetadataProvider + BlockHashReader>(
    provider: &P,
    write: SnapWrite,
    target: u64,
    max_blocks: u64,
) -> Result<Vec<SealedHeader<P::Header>>, SnapSyncError> {
    let progress = provider.catch_up_progress(write)?.ok_or(SnapSyncError::NoCatchUpProgress)?;
    let applied = progress.applied();
    let canonical = provider
        .sealed_header(applied.number)?
        .ok_or(SnapSyncError::MissingHeader { block: applied.number })?;
    if canonical.hash() != applied.hash {
        return Err(SnapSyncError::ForkedBlock { expected: applied.hash, got: canonical.hash() })
    }
    // Pending ranges are proved against the pivot, so no list past it applies.
    let target = target.min(provider.authorize_canonical_snap_write(write)?.pivot().number);
    if target <= applied.number {
        return Ok(Vec::new())
    }

    let end = target.min(applied.number.saturating_add(max_blocks));
    let headers = provider.sealed_headers_range(progress.next()..=end)?;
    if headers.is_empty() {
        return Err(SnapSyncError::MissingHeader { block: progress.next() })
    }
    // A list is only authenticated by the header of the block it belongs to, so a gap in them
    // ends the run as surely as a gap in the lists themselves.
    for (number, header) in (progress.next()..=end).zip(&headers) {
        if header.number() != number {
            return Err(SnapSyncError::MissingHeader { block: number })
        }
    }
    Ok(headers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_utils::{
            account, hashed_factory, key, state_root, verified_range, BalChain, ScriptedSnapClient,
            SnapStateSnapshot,
        },
        SnapAccountStore, SnapGeneration,
    };
    use alloy_eip7928::{AccountChanges, BalanceChange, BlockAccessIndex};
    use alloy_primitives::{keccak256, Address, B256, U256};
    use reth_db_api::{cursor::DbCursorRO, tables, transaction::DbTx};
    use reth_network_p2p::{error::PeerRequestResult, snap::client::SnapResponse};
    use reth_provider::{
        test_utils::{insert_headers, MockNodeTypesWithDB},
        ProviderFactory,
    };
    use reth_trie_common::{TrieAccount, EMPTY_ROOT_HASH};
    use std::sync::Arc;

    type Factory = ProviderFactory<MockNodeTypesWithDB>;
    type CatchUp = BlockAccessListCatchUp<Arc<ScriptedSnapClient>, Factory>;

    // The account every fixture list credits.
    const CHANGED: Address = Address::repeat_byte(0xaa);
    // Pivot the fixture chain is anchored to.
    const PIVOT: u64 = 2;

    fn accounts() -> Vec<(B256, TrieAccount)> {
        let mut accounts = vec![(key(1), account(1)), (keccak256(CHANGED), account(2))];
        accounts.sort_by_key(|(hashed_address, _)| *hashed_address);
        accounts
    }

    // A list crediting `CHANGED` with `balance`.
    fn credit(balance: u64) -> Vec<AccountChanges> {
        vec![AccountChanges::new(CHANGED)
            .with_balance_change(BalanceChange::new(BlockAccessIndex::new(1), U256::from(balance)))]
    }

    // Three blocks after the pivot, crediting the account 10, 20 and 30.
    fn chain() -> BalChain {
        BalChain::new(PIVOT, [credit(10), credit(20), credit(30)])
    }

    // An attempt started at `chain`'s pivot, with every account downloaded and nothing applied,
    // then moved to the chain's last block.
    fn started(chain: &BalChain, accounts: &[(B256, TrieAccount)]) -> (Factory, SnapWrite) {
        let factory = hashed_factory();
        insert_headers(&factory, &chain.headers);
        let provider = factory.database_provider_rw().unwrap();
        let write = provider.start_snap_attempt(chain.generation(state_root(accounts))).unwrap();
        provider.start_account_coverage(write).unwrap();
        let range = verified_range(accounts, 0..accounts.len(), B256::ZERO, &[]);
        provider.commit_account_range(write, &range, Default::default(), Vec::new()).unwrap();
        let tip = SnapGeneration::new(chain.tip(), state_root(accounts));
        let write = provider.advance_snap_pivot(write, tip).unwrap();
        provider.commit().unwrap();
        (factory, write)
    }

    // The reorg replaces the pivot alone: the applied block and the one the lists carry the state
    // through stay canonical, so only the pivot check can refuse what follows. Ranges downloaded
    // at that pivot hold a balance change the replacement branch never made.
    fn pivot_reorg_fixture() -> (Factory, SnapWrite, BalChain, BalChain) {
        let chain = BalChain::new(PIVOT, [Vec::new(), credit(30)]);
        let replacement = BalChain::new(PIVOT, [Vec::new(), Vec::new()]);
        assert_eq!(chain.block(1), replacement.block(1));
        assert_ne!(chain.tip(), replacement.tip());
        let mut accounts = accounts();
        let factory = hashed_factory();
        insert_headers(&factory, &chain.headers);
        let provider = factory.database_provider_rw().unwrap();
        let write = provider.start_snap_attempt(chain.generation(state_root(&accounts))).unwrap();
        provider.start_account_coverage(write).unwrap();
        accounts.iter_mut().find(|(key, _)| *key == keccak256(CHANGED)).unwrap().1.balance =
            U256::from(30);
        let write = provider
            .advance_snap_pivot(write, SnapGeneration::new(chain.tip(), state_root(&accounts)))
            .unwrap();
        let range = verified_range(&accounts, 0..accounts.len(), B256::ZERO, &[]);
        provider.commit_account_range(write, &range, Default::default(), Vec::new()).unwrap();
        provider.commit().unwrap();
        (factory, write, chain, replacement)
    }

    fn catch_up(
        responses: impl IntoIterator<Item = PeerRequestResult<SnapResponse>>,
        factory: Factory,
    ) -> (Arc<ScriptedSnapClient>, CatchUp) {
        catch_up_with(responses, factory, DEFAULT_CATCH_UP_BLOCKS)
    }

    fn catch_up_with(
        responses: impl IntoIterator<Item = PeerRequestResult<SnapResponse>>,
        factory: Factory,
        max_blocks: u64,
    ) -> (Arc<ScriptedSnapClient>, CatchUp) {
        let client = Arc::new(ScriptedSnapClient::new(responses));
        let catch_up = BlockAccessListCatchUp::new(Arc::clone(&client), factory, Runtime::test());
        (client, catch_up.with_max_blocks(max_blocks))
    }

    async fn applied(catch_up: &mut CatchUp, write: SnapWrite, target: u64) -> CatchUpProgress {
        match catch_up.next(write, target).await.unwrap() {
            CatchUpStep::Applied { progress, .. } => progress,
            step => panic!("expected an application, got {step:?}"),
        }
    }

    fn balance(factory: &Factory) -> U256 {
        let provider = factory.database_provider_ro().unwrap();
        provider
            .tx_ref()
            .get::<tables::HashedAccounts>(keccak256(CHANGED))
            .unwrap()
            .unwrap()
            .balance
    }

    // The fixture accounts with `CHANGED` holding `balance`, plus `extra`.
    fn accounts_with(
        balance: u64,
        extra: impl IntoIterator<Item = (B256, TrieAccount)>,
    ) -> Vec<(B256, TrieAccount)> {
        let mut accounts: Vec<_> = accounts().into_iter().chain(extra).collect();
        accounts.iter_mut().find(|(key, _)| *key == keccak256(CHANGED)).unwrap().1.balance =
            U256::from(balance);
        accounts.sort_by_key(|(hashed_address, _)| *hashed_address);
        accounts
    }

    // Root of the account trie the downloaded state holds, none of the fixture accounts having
    // storage.
    fn downloaded_root(factory: &Factory) -> B256 {
        let provider = factory.database_provider_ro().unwrap();
        let mut cursor = provider.tx_ref().cursor_read::<tables::HashedAccounts>().unwrap();
        let accounts: Vec<_> = cursor
            .walk(None)
            .unwrap()
            .map(|entry| {
                let (hashed_address, account) = entry.unwrap();
                (hashed_address, account.into_trie_account(EMPTY_ROOT_HASH))
            })
            .collect();
        state_root(&accounts)
    }

    // An attempt at `chain`'s pivot holding `accounts[..served]`, proved against `accounts`.
    fn partially_downloaded(
        chain: &BalChain,
        accounts: &[(B256, TrieAccount)],
        served: usize,
    ) -> (Factory, SnapWrite) {
        let factory = hashed_factory();
        insert_headers(&factory, &chain.headers);
        let provider = factory.database_provider_rw().unwrap();
        let write = provider.start_snap_attempt(chain.generation(state_root(accounts))).unwrap();
        provider.start_account_coverage(write).unwrap();
        let range =
            verified_range(accounts, 0..served, B256::ZERO, &[B256::ZERO, accounts[served - 1].0]);
        provider.commit_account_range(write, &range, Default::default(), Vec::new()).unwrap();
        provider.commit().unwrap();
        (factory, write)
    }

    // Moves the attempt to the block `nth` after the chain's pivot, whose state is `accounts`.
    fn advance(
        factory: &Factory,
        write: SnapWrite,
        chain: &BalChain,
        nth: usize,
        accounts: &[(B256, TrieAccount)],
    ) -> SnapWrite {
        let provider = factory.database_provider_rw().unwrap();
        let generation = SnapGeneration::new(chain.block(nth), state_root(accounts));
        let write = provider.advance_snap_pivot(write, generation).unwrap();
        provider.commit().unwrap();
        write
    }

    // Commits the rest of the trie from the coverage cursor, proved against `accounts`.
    fn download_rest(factory: &Factory, write: SnapWrite, accounts: &[(B256, TrieAccount)]) {
        let provider = factory.database_provider_rw().unwrap();
        let origin = provider.account_coverage(write).unwrap().unwrap().next().unwrap();
        let served = accounts.iter().position(|(key, _)| *key >= origin).unwrap()..accounts.len();
        let range = verified_range(accounts, served, origin, &[origin, accounts.last().unwrap().0]);
        provider.commit_account_range(write, &range, Default::default(), Vec::new()).unwrap();
        provider.commit().unwrap();
    }

    #[tokio::test]
    async fn every_list_a_response_carries_is_applied_in_block_order() {
        let chain = chain();
        let (factory, write) = started(&chain, &accounts());
        let (client, mut catch_up) =
            catch_up([chain.response(1, [Some(1), Some(2), Some(3)])], factory.clone());

        let progress = applied(&mut catch_up, write, PIVOT + 3).await;

        assert_eq!(progress.applied(), chain.block(3));
        // The last block's credit is the one that survives.
        assert_eq!(balance(&factory), U256::from(30));
        assert_eq!(
            *client.block_requests(),
            [(1..=3).map(|nth| chain.block(nth).hash).collect::<Vec<_>>()]
        );
        assert!(matches!(catch_up.next(write, PIVOT + 3).await.unwrap(), CatchUpStep::Complete));
    }

    #[tokio::test]
    async fn a_list_a_peer_leaves_out_holds_back_the_blocks_after_it() {
        let chain = chain();
        let (factory, write) = started(&chain, &accounts());
        let (client, mut catch_up) = catch_up(
            [chain.response(1, [Some(1), None, Some(3)]), chain.response(2, [Some(2), Some(3)])],
            factory.clone(),
        );

        // Only the block before the gap applies, although a later list was authenticated.
        let progress = applied(&mut catch_up, write, PIVOT + 3).await;
        assert_eq!(progress.applied(), chain.block(1));
        assert_eq!(balance(&factory), U256::from(10));

        let progress = applied(&mut catch_up, write, PIVOT + 3).await;
        assert_eq!(progress.applied(), chain.block(3));
        assert_eq!(balance(&factory), U256::from(30));
        // The gap is asked for again, and nothing before it is.
        assert_eq!(client.block_requests()[1], [chain.block(2).hash, chain.block(3).hash]);
    }

    #[tokio::test]
    async fn a_response_cut_short_leaves_the_rest_pending() {
        let chain = chain();
        let (factory, write) = started(&chain, &accounts());
        // A peer that stops at its soft byte limit answers fewer blocks than it was asked for.
        let (client, mut catch_up) = catch_up(
            [chain.response(1, [Some(1), Some(2)]), chain.response(2, [Some(3)])],
            factory.clone(),
        );

        let progress = applied(&mut catch_up, write, PIVOT + 3).await;
        assert_eq!(progress.applied(), chain.block(2));

        let progress = applied(&mut catch_up, write, PIVOT + 3).await;
        assert_eq!(progress.applied(), chain.block(3));
        assert_eq!(balance(&factory), U256::from(30));
        // Only the suffix the response left off is asked for again.
        assert_eq!(client.block_requests()[1], [chain.block(3).hash]);
    }

    #[tokio::test]
    async fn a_reply_repeating_an_applied_list_cannot_apply_it_again() {
        let chain = chain();
        let (factory, write) = started(&chain, &accounts());
        // The first block's list, served again for every attempt at the second block.
        let duplicate = std::iter::repeat_with(|| chain.response(2, [Some(1)])).take(4);
        let (_, mut catch_up) = catch_up_with(
            std::iter::once(chain.response(1, [Some(1)])).chain(duplicate),
            factory.clone(),
            1,
        );
        applied(&mut catch_up, write, PIVOT + 3).await;

        // It belongs to a block the applied state already covers, so it authenticates against
        // nothing the request asked for.
        let repeated = catch_up.next(write, PIVOT + 3).await;

        assert!(matches!(repeated, Err(SnapSyncError::Request(_))));
        assert_eq!(balance(&factory), U256::from(10));
    }

    #[tokio::test]
    async fn a_peer_holding_no_list_for_the_next_block_applies_nothing() {
        let chain = chain();
        let (factory, write) = started(&chain, &accounts());
        let (_, mut catch_up) = catch_up([chain.response(1, [None, None, None])], factory.clone());

        let step = catch_up.next(write, PIVOT + 3).await.unwrap();

        assert!(matches!(step, CatchUpStep::Unavailable { .. }));
        assert_eq!(balance(&factory), U256::from(1));
    }

    #[tokio::test]
    async fn a_block_that_changes_nothing_is_still_carried_past() {
        let chain = BalChain::new(PIVOT, [Vec::new()]);
        let (factory, write) = started(&chain, &accounts());
        let (_, mut catch_up) = catch_up([chain.response(1, [Some(1)])], factory.clone());

        let progress = applied(&mut catch_up, write, PIVOT + 1).await;

        assert_eq!(progress.applied(), chain.block(1));
        assert_eq!(balance(&factory), U256::from(1));
    }

    #[tokio::test]
    async fn only_the_blocks_a_request_can_carry_are_asked_for() {
        let chain = chain();
        let (factory, write) = started(&chain, &accounts());
        let (client, mut catch_up) = catch_up_with([chain.response(1, [Some(1)])], factory, 1);

        applied(&mut catch_up, write, PIVOT + 3).await;

        assert_eq!(*client.block_requests(), [vec![chain.block(1).hash]]);
    }

    #[tokio::test]
    async fn an_unbounded_request_asks_for_every_block_through_the_target() {
        let chain = chain();
        let (factory, write) = started(&chain, &accounts());
        let (client, mut catch_up) =
            catch_up_with([chain.response(1, [Some(1), Some(2), Some(3)])], factory, u64::MAX);

        let progress = applied(&mut catch_up, write, PIVOT + 3).await;

        assert_eq!(progress.applied(), chain.block(3));
        assert_eq!(
            *client.block_requests(),
            [(1..=3).map(|nth| chain.block(nth).hash).collect::<Vec<_>>()]
        );
    }

    #[tokio::test]
    async fn a_target_already_applied_needs_no_request() {
        let chain = chain();
        let (factory, write) = started(&chain, &accounts());
        let (client, mut catch_up) = catch_up([], factory);

        assert!(matches!(catch_up.next(write, PIVOT).await.unwrap(), CatchUpStep::Complete));
        assert!(client.block_requests().is_empty());
    }

    #[tokio::test]
    async fn a_target_past_the_pivot_is_held_at_it() {
        let chain = chain();
        let factory = hashed_factory();
        insert_headers(&factory, &chain.headers);
        let provider = factory.database_provider_rw().unwrap();
        let write = provider.start_snap_attempt(chain.generation(state_root(&accounts()))).unwrap();
        provider.commit().unwrap();
        let (client, mut catch_up) = catch_up([], factory);

        assert!(matches!(catch_up.next(write, PIVOT + 3).await.unwrap(), CatchUpStep::Complete));
        assert!(client.block_requests().is_empty());
    }

    #[tokio::test]
    async fn an_applied_block_the_canonical_chain_no_longer_holds_is_reported() {
        let chain = chain();
        let accounts = accounts();
        // The chain the node holds instead, forking before the pivot as a reorg leaves it.
        let reorged = BalChain::new(0, [credit(1), credit(2), credit(3)]);
        let factory = hashed_factory();
        insert_headers(&factory, &reorged.headers);
        let provider = factory.database_provider_rw().unwrap();
        let write = provider.start_snap_attempt(chain.generation(state_root(&accounts))).unwrap();
        provider.start_account_coverage(write).unwrap();
        provider.commit().unwrap();
        let (_, mut catch_up) = catch_up([], factory);

        let forked = catch_up.next(write, PIVOT + 3).await;

        assert!(matches!(forked, Err(SnapSyncError::ForkedBlock { .. })));
    }

    #[tokio::test]
    async fn a_pivot_the_canonical_chain_no_longer_holds_is_reported() {
        let (factory, write, chain, replacement) = pivot_reorg_fixture();
        let before = SnapStateSnapshot::read(&factory.database_provider_ro().unwrap());
        replacement.replace_tip(&factory);
        let (client, mut catch_up) = catch_up([], factory.clone());

        // Even an already-applied target must reject the orphan instead of reporting completion.
        for target in [PIVOT, PIVOT + 2] {
            assert!(matches!(
                catch_up.next(write, target).await,
                Err(SnapSyncError::NonCanonicalBlock { block, hash })
                    if block == PIVOT + 2 && hash == chain.tip().hash
            ));
        }
        assert!(client.block_requests().is_empty());
        let provider = factory.database_provider_ro().unwrap();
        // The block whose list would apply next is untouched by the reorg.
        assert_eq!(provider.block_hash(PIVOT + 1).unwrap(), Some(chain.block(1).hash));
        assert_eq!(provider.block_hash(PIVOT).unwrap(), Some(chain.block(0).hash));
        assert_eq!(provider.catch_up_progress(write).unwrap().unwrap().applied(), chain.block(0));
        assert_eq!(SnapStateSnapshot::read(&provider), before);
    }

    #[tokio::test]
    async fn a_pivot_reorg_during_a_bal_request_changes_nothing() {
        let (factory, write, chain, replacement) = pivot_reorg_fixture();
        let before = SnapStateSnapshot::read(&factory.database_provider_ro().unwrap());
        let reorg_factory = factory.clone();
        let replacement_hash = replacement.tip().hash;
        let client = Arc::new(
            ScriptedSnapClient::new([chain.response(1, [Some(1)])])
                .on_block_request(move || replacement.replace_tip(&reorg_factory)),
        );
        let mut catch_up =
            BlockAccessListCatchUp::new(client.clone(), factory.clone(), Runtime::test());

        // The list the peer served belongs to a block the reorg left canonical, so refusing it is
        // the pivot check's doing.
        assert!(matches!(
            catch_up.next(write, PIVOT + 2).await,
            Err(SnapSyncError::NonCanonicalBlock { block, hash })
                if block == PIVOT + 2 && hash == chain.tip().hash
        ));

        assert_eq!(*client.block_requests(), [vec![chain.block(1).hash, chain.tip().hash]]);
        let provider = factory.database_provider_ro().unwrap();
        assert_eq!(provider.block_hash(PIVOT + 1).unwrap(), Some(chain.block(1).hash));
        assert_eq!(provider.block_hash(PIVOT + 2).unwrap(), Some(replacement_hash));
        assert_eq!(provider.catch_up_progress(write).unwrap().unwrap().applied(), chain.block(0));
        assert_eq!(SnapStateSnapshot::read(&provider), before);
    }

    #[tokio::test]
    async fn a_reply_to_an_earlier_request_is_ignored() {
        let chain = chain();
        let (factory, write) = started(&chain, &accounts());
        // A reply carrying another request's id, as a delayed one does.
        let stale = chain.response(99, [Some(1), Some(2), Some(3)]);
        let (_, mut catch_up) =
            catch_up([stale, chain.response(1, [Some(1), Some(2), Some(3)])], factory.clone());

        // The retry is what the authenticated lists arrive on.
        let progress = applied(&mut catch_up, write, PIVOT + 3).await;

        assert_eq!(progress.applied(), chain.block(3));
        assert_eq!(balance(&factory), U256::from(30));
    }

    #[tokio::test]
    async fn downloads_split_across_pivots_converge_on_the_latest_pivot_state() {
        const FAR: B256 = B256::repeat_byte(0xfe);
        let chain = chain();
        let at = |balance| accounts_with(balance, [(FAR, account(3))]);
        assert_eq!(at(1).last().unwrap().0, FAR);
        // The pivot serves every account but the last.
        let (factory, write) = partially_downloaded(&chain, &at(1), 2);
        let (_, mut catch_up) =
            catch_up([chain.response(1, [Some(1)]), chain.response(2, [Some(2)])], factory.clone());

        let write = advance(&factory, write, &chain, 1, &at(10));
        applied(&mut catch_up, write, PIVOT + 3).await;
        assert_eq!(balance(&factory), U256::from(10));
        let write = advance(&factory, write, &chain, 2, &at(20));

        // A range proved against the previous root cannot complete the new one's coverage.
        let provider = factory.database_provider_rw().unwrap();
        let origin = provider.account_coverage(write).unwrap().unwrap().next().unwrap();
        let stale = verified_range(&at(10), 2..3, origin, &[origin, FAR]);
        assert!(matches!(
            provider.commit_account_range(write, &stale, Default::default(), Vec::new()),
            Err(SnapSyncError::RootMismatch { .. })
        ));
        drop(provider);
        download_rest(&factory, write, &at(20));

        // Lists stop at the latest pivot, where the downloaded state now sits.
        let progress = applied(&mut catch_up, write, PIVOT + 3).await;
        assert_eq!(progress.applied(), chain.block(2));
        assert!(matches!(catch_up.next(write, PIVOT + 3).await.unwrap(), CatchUpStep::Complete));
        assert_eq!(downloaded_root(&factory), state_root(&at(20)));
    }

    #[tokio::test]
    async fn an_account_lists_reach_before_its_range_is_taken_from_the_new_root() {
        let chain = chain();
        let at = |balance| accounts_with(balance, []);
        // Only the account before the changed one is downloaded at the pivot.
        assert_eq!(at(1)[1].0, keccak256(CHANGED));
        let (factory, write) = partially_downloaded(&chain, &at(1), 1);
        let write = advance(&factory, write, &chain, 2, &at(20));
        let (_, mut catch_up) = catch_up([chain.response(1, [Some(1), Some(2)])], factory.clone());

        // The lists have no base to change yet, so they leave the account to its range.
        let progress = applied(&mut catch_up, write, PIVOT + 3).await;
        assert_eq!(progress.applied(), chain.block(2));
        let provider = factory.database_provider_ro().unwrap();
        assert_eq!(
            provider.tx_ref().get::<tables::HashedAccounts>(keccak256(CHANGED)).unwrap(),
            None
        );
        drop(provider);

        download_rest(&factory, write, &at(20));

        assert_eq!(balance(&factory), U256::from(20));
        assert_eq!(downloaded_root(&factory), state_root(&at(20)));
    }
}

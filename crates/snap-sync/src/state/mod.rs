//! Coordinates authenticated account ranges with their incremental storage and code downloads.

pub(crate) mod account;
pub(crate) mod bytecode;
pub(crate) mod storage;

pub use account::{
    AccountCoverage, AccountRangeDownload, AccountRangeStep, SnapAccountStore, VerifiedRange,
};
pub use bytecode::{BytecodeDownload, BytecodeStep, SnapBytecodeStore, DEFAULT_CODE_HASHES};
pub use storage::{
    SnapStorageStore, StorageChunk, StorageProgress, StorageRangeDownload, StorageRangeStep,
    DEFAULT_STORAGE_ACCOUNTS,
};

use crate::{SnapDownloadProgress, SnapPhase, SnapStateStore, SnapSyncError, SnapSyncProvider};
use reth_network_p2p::snap::client::SnapClient;
use reth_provider::DatabaseProviderFactory;
use reth_storage_api::{MetadataProvider, StageCheckpointReader};
use reth_tasks::Runtime;

/// Downloads dependencies before extending the durable account coverage.
#[derive(Debug)]
pub struct StateDownloader<'a, C, F> {
    accounts: AccountRangeDownload<&'a C, F>,
    storage: StorageRangeDownload<&'a C, F>,
    bytecode: BytecodeDownload<&'a C, F>,
    store: SnapStateStore<'a, F>,
}

impl<'a, C, F: Clone> StateDownloader<'a, C, F> {
    /// Creates a coordinator over the same persisted attempt as its domain downloaders.
    pub fn new(client: &'a C, factory: &'a F, runtime: Runtime) -> Self {
        Self {
            accounts: AccountRangeDownload::new(client, factory.clone(), runtime.clone()),
            storage: StorageRangeDownload::new(client, factory.clone(), runtime.clone()),
            bytecode: BytecodeDownload::new(client, factory.clone(), runtime),
            store: SnapStateStore::new(factory),
        }
    }

    /// Commits at most `budget` ranges, retaining partial dependency progress on unavailable peers.
    pub async fn run(
        &mut self,
        mut generation: SnapDownloadProgress,
        budget: RangeBudget,
    ) -> Result<StateDownloadOutcome, SnapSyncError>
    where
        C: SnapClient,
        F: DatabaseProviderFactory<Provider: MetadataProvider + StageCheckpointReader> + 'static,
        F::ProviderRW: SnapSyncProvider,
    {
        generation.ensure_phase(SnapPhase::Accounts)?;
        // BALs update only covered accounts. Extending coverage before catch-up finishes would
        // apply old changes to accounts already downloaded at the new root.
        if generation.next_block <= generation.target_block {
            return Err(SnapSyncError::InvalidGeneration(
                "account coverage cannot advance during BAL catch-up".into(),
            ));
        }
        for _ in 0..budget.ranges() {
            let range = match self.accounts.next().await? {
                Some(AccountRangeStep::Verified(range)) => range,
                Some(AccountRangeStep::Unavailable { .. }) => {
                    return Ok(StateDownloadOutcome::Unavailable { generation })
                }
                None => return Ok(StateDownloadOutcome::Complete { generation: self.progress()? }),
            };
            loop {
                match self.storage.next(&range).await? {
                    StorageRangeStep::Complete => break,
                    StorageRangeStep::Committed(_) => {}
                    StorageRangeStep::Unavailable { .. } => {
                        return Ok(StateDownloadOutcome::Unavailable { generation })
                    }
                }
            }
            loop {
                match self.bytecode.next(&range).await? {
                    BytecodeStep::Complete => break,
                    BytecodeStep::Committed { .. } => {}
                    BytecodeStep::Unavailable { .. } => {
                        return Ok(StateDownloadOutcome::Unavailable { generation })
                    }
                }
            }
            let coverage = self.accounts.commit(range, Default::default(), Vec::new()).await?;
            generation = self.progress()?;
            if coverage.is_complete() {
                return Ok(StateDownloadOutcome::Complete { generation });
            }
        }
        Ok(StateDownloadOutcome::Paused { generation })
    }

    fn progress(&self) -> Result<SnapDownloadProgress, SnapSyncError>
    where
        F: DatabaseProviderFactory<Provider: MetadataProvider + StageCheckpointReader>,
    {
        self.store.interrupted_generation()?.ok_or(SnapSyncError::StaleGeneration)
    }
}

/// Terminal result of one state-download attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateDownloadOutcome {
    /// Every account dependency was committed and the generation entered BAL catch-up.
    Complete {
        /// Updated durable generation.
        generation: SnapDownloadProgress,
    },
    /// No eligible peer currently serves the first uncommitted range.
    Unavailable {
        /// Last fully committed generation position.
        generation: SnapDownloadProgress,
    },
    /// The range budget was spent while account ranges remained.
    Paused {
        /// Last fully committed generation position.
        generation: SnapDownloadProgress,
    },
}

/// Account ranges committed before yielding so callers can re-anchor within served BAL history.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RangeBudget(usize);

impl RangeBudget {
    /// Downloads until the state completes or peers stop serving it.
    pub const UNBOUNDED: Self = Self(usize::MAX);

    /// Returns control after `ranges` committed account ranges.
    pub const fn new(ranges: usize) -> Self {
        Self(ranges)
    }

    /// Returns the number of account ranges left in the budget.
    pub const fn ranges(self) -> usize {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::ScriptedSnapClient as TestSnapClient;
    use alloy_primitives::{keccak256, Bytes, B256, KECCAK256_EMPTY, U256};
    use reth_db_api::{cursor::DbDupCursorRO, tables, transaction::DbTx};
    use reth_eth_wire_types::snap::{
        AccountData, AccountRangeMessage, ByteCodesMessage, StorageData, StorageRangesMessage,
    };
    use reth_network_p2p::{error::PeerRequestResult, snap::client::SnapResponse};
    use reth_network_peers::{PeerId, WithPeerId};
    use reth_trie_common::{
        proof::ProofRetainer, HashBuilder, Nibbles, TrieAccount, EMPTY_ROOT_HASH,
    };

    type AccountEntry = (B256, TrieAccount);

    fn response(peer_id: PeerId, response: SnapResponse) -> PeerRequestResult<SnapResponse> {
        Ok(WithPeerId::new(peer_id, response))
    }

    fn empty_account(nonce: u64) -> TrieAccount {
        TrieAccount {
            nonce,
            balance: U256::from(1),
            storage_root: EMPTY_ROOT_HASH,
            code_hash: KECCAK256_EMPTY,
        }
    }

    fn root_and_proof(accounts: &[AccountEntry], targets: &[B256]) -> (B256, Vec<Bytes>) {
        let targets = targets.iter().copied().map(Nibbles::unpack).collect();
        let mut builder = HashBuilder::default().with_proof_retainer(ProofRetainer::new(targets));
        for (hash, account) in accounts {
            builder.add_leaf(Nibbles::unpack(*hash), &alloy_rlp::encode(account));
        }
        let root = builder.root();
        let proof =
            builder.take_proof_nodes().into_nodes_sorted().into_iter().map(|(_, node)| node);
        (root, proof.collect())
    }

    fn trie_root(entries: impl IntoIterator<Item = (B256, Vec<u8>)>) -> B256 {
        let mut builder = HashBuilder::default();
        for (hash, value) in entries {
            builder.add_leaf(Nibbles::unpack(hash), &value);
        }
        builder.root()
    }

    #[tokio::test]
    async fn empty_root_completes_state_download() {
        let factory = crate::test_utils::hashed_factory();
        let generation = SnapDownloadProgress::new(10, B256::repeat_byte(1), EMPTY_ROOT_HASH);
        SnapStateStore::new(&factory).begin_generation(generation).unwrap();
        let client = TestSnapClient::new([response(
            PeerId::random(),
            SnapResponse::AccountRange(AccountRangeMessage {
                request_id: 1,
                accounts: Vec::new(),
                proof: Vec::new(),
            }),
        )]);

        let outcome = StateDownloader::new(&client, &factory, Runtime::test())
            .run(generation, RangeBudget::UNBOUNDED)
            .await
            .unwrap();

        let StateDownloadOutcome::Complete { generation } = outcome else {
            panic!("completed state download")
        };
        assert_eq!(generation.phase, SnapPhase::BlockAccessLists);
    }

    #[tokio::test]
    async fn unavailable_range_preserves_coverage() {
        let factory = crate::test_utils::hashed_factory();
        let generation = SnapDownloadProgress::new(10, B256::repeat_byte(1), B256::repeat_byte(2));
        SnapStateStore::new(&factory).begin_generation(generation).unwrap();
        let first = PeerId::random();
        let second = PeerId::random();
        let client = TestSnapClient::new([
            response(
                first,
                SnapResponse::AccountRange(AccountRangeMessage {
                    request_id: 1,
                    accounts: Vec::new(),
                    proof: Vec::new(),
                }),
            ),
            response(
                second,
                SnapResponse::AccountRange(AccountRangeMessage {
                    request_id: 2,
                    accounts: Vec::new(),
                    proof: Vec::new(),
                }),
            ),
        ]);

        let outcome = StateDownloader::new(&client, &factory, Runtime::test())
            .run(generation, RangeBudget::UNBOUNDED)
            .await
            .unwrap();

        assert_eq!(outcome, StateDownloadOutcome::Unavailable { generation });
        assert_eq!(client.origins().len(), 1);
    }

    #[tokio::test]
    async fn commits_account_storage_and_bytecode_as_one_batch() {
        let factory = crate::test_utils::hashed_factory();
        let account_hash = B256::repeat_byte(0x11);
        let slot_hash = B256::repeat_byte(0x22);
        let slot_value = U256::from(7);
        let storage_root = trie_root([(slot_hash, alloy_rlp::encode(slot_value))]);
        let code = Bytes::from_static(&[0x60, 0x00]);
        let code_hash = keccak256(&code);
        let account = TrieAccount { nonce: 3, balance: U256::from(4), storage_root, code_hash };
        let state_root = trie_root([(account_hash, alloy_rlp::encode(account))]);
        let generation = SnapDownloadProgress::new(10, B256::repeat_byte(1), state_root);
        SnapStateStore::new(&factory).begin_generation(generation).unwrap();
        let peer = PeerId::random();
        let client = TestSnapClient::new([
            response(
                peer,
                SnapResponse::AccountRange(AccountRangeMessage {
                    request_id: 1,
                    accounts: vec![AccountData::from_trie_account(account_hash, &account)],
                    proof: Vec::new(),
                }),
            ),
            response(
                peer,
                SnapResponse::StorageRanges(StorageRangesMessage {
                    request_id: 1,
                    slots: vec![vec![StorageData::from_value(slot_hash, slot_value)]],
                    proof: Vec::new(),
                }),
            ),
            response(
                peer,
                SnapResponse::ByteCodes(ByteCodesMessage {
                    request_id: 1,
                    codes: vec![code.clone()],
                }),
            ),
        ]);

        let outcome = StateDownloader::new(&client, &factory, Runtime::test())
            .run(generation, RangeBudget::UNBOUNDED)
            .await
            .unwrap();

        assert!(matches!(outcome, StateDownloadOutcome::Complete { .. }));
        let provider = factory.database_provider_ro().unwrap();
        assert_eq!(
            provider.tx_ref().get::<tables::HashedAccounts>(account_hash).unwrap().unwrap().nonce,
            3
        );
        assert_eq!(
            provider
                .tx_ref()
                .get::<tables::Bytecodes>(code_hash)
                .unwrap()
                .unwrap()
                .original_bytes(),
            code
        );
        let mut cursor = provider.tx_ref().cursor_dup_read::<tables::HashedStorages>().unwrap();
        assert_eq!(
            cursor.seek_by_key_subkey(account_hash, slot_hash).unwrap().unwrap().value,
            slot_value
        );
    }

    #[tokio::test]
    async fn skips_empty_storage_accounts_in_a_mixed_range() {
        let factory = crate::test_utils::hashed_factory();
        let empty_hash = B256::repeat_byte(0x11);
        let stored_hash = B256::repeat_byte(0x22);
        let slot_hash = B256::repeat_byte(0x33);
        let slot_value = U256::from(7);
        let storage_root = trie_root([(slot_hash, alloy_rlp::encode(slot_value))]);
        let empty = empty_account(1);
        let stored = TrieAccount {
            nonce: 2,
            balance: U256::from(3),
            storage_root,
            code_hash: KECCAK256_EMPTY,
        };
        let state_root = trie_root([
            (empty_hash, alloy_rlp::encode(empty)),
            (stored_hash, alloy_rlp::encode(stored)),
        ]);
        let generation = SnapDownloadProgress::new(10, B256::repeat_byte(1), state_root);
        SnapStateStore::new(&factory).begin_generation(generation).unwrap();
        let peer = PeerId::random();
        let client = TestSnapClient::new([
            response(
                peer,
                SnapResponse::AccountRange(AccountRangeMessage {
                    request_id: 1,
                    accounts: vec![
                        AccountData::from_trie_account(empty_hash, &empty),
                        AccountData::from_trie_account(stored_hash, &stored),
                    ],
                    proof: Vec::new(),
                }),
            ),
            response(
                peer,
                SnapResponse::StorageRanges(StorageRangesMessage {
                    request_id: 1,
                    slots: vec![vec![StorageData::from_value(slot_hash, slot_value)]],
                    proof: Vec::new(),
                }),
            ),
        ]);

        let outcome = StateDownloader::new(&client, &factory, Runtime::test())
            .run(generation, RangeBudget::UNBOUNDED)
            .await
            .unwrap();

        assert!(matches!(outcome, StateDownloadOutcome::Complete { .. }));
        let provider = factory.database_provider_ro().unwrap();
        let mut cursor = provider.tx_ref().cursor_dup_read::<tables::HashedStorages>().unwrap();
        assert_eq!(
            cursor.seek_by_key_subkey(stored_hash, slot_hash).unwrap().unwrap().value,
            slot_value
        );
    }

    #[tokio::test]
    async fn spent_budget_pauses_before_the_next_range() {
        let factory = crate::test_utils::hashed_factory();
        let first = (B256::repeat_byte(0x11), empty_account(1));
        let second = (B256::repeat_byte(0x22), empty_account(2));
        let (state_root, proof) = root_and_proof(&[first, second], &[first.0]);
        let generation = SnapDownloadProgress::new(10, B256::repeat_byte(1), state_root);
        SnapStateStore::new(&factory).begin_generation(generation).unwrap();
        let client = TestSnapClient::new([response(
            PeerId::random(),
            SnapResponse::AccountRange(AccountRangeMessage {
                request_id: 1,
                accounts: vec![AccountData::from_trie_account(first.0, &first.1)],
                proof,
            }),
        )]);

        let outcome = StateDownloader::new(&client, &factory, Runtime::test())
            .run(generation, RangeBudget::new(1))
            .await
            .unwrap();

        let StateDownloadOutcome::Paused { generation } = outcome else {
            panic!("paused state download")
        };
        assert_eq!(generation.phase, SnapPhase::Accounts);
        // The proof covers the gap up to the next subtree, beyond the returned account.
        let mut next_account = B256::ZERO;
        next_account[0] = 0x20;
        assert_eq!(generation.next_account, next_account);
        assert_eq!(
            SnapStateStore::new(&factory).interrupted_generation().unwrap(),
            Some(generation)
        );
        // Only the first range was requested, so the peer never saw a continuation.
        assert_eq!(client.origins().len(), 1);
    }
}

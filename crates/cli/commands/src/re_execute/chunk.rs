//! Independent canonical inputs and versioned state for reset-independent replay.

use alloy_consensus::BlockHeader;
use alloy_primitives::{Address, B256, U256};
use parking_lot::Mutex;
use reth_cli_util::cancellation::CancellationToken;
use reth_consensus::FullConsensus;
use reth_db_api::models::{AccountBeforeTx, BlockNumberAddress};
use reth_evm::{ConfigureEvm, Database};
use reth_primitives_traits::{
    Account, Block as BlockTrait, BlockBody, BlockTy, RecoveredBlock, StorageEntry,
};
use reth_provider::{ProviderError, ProviderResult};
use reth_revm::{bytecode::Bytecode, state::AccountInfo};
use reth_storage_api::{ChangeSetReader, StorageChangeSetReader};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    ops::{Range, RangeBounds},
    sync::Arc,
};
use wincode::{SchemaRead, SchemaWrite};

#[cfg(test)]
mod tests;

/// A finite exclusive-end chunk; all versions and expectations originate in the archive DB.
#[derive(Debug, SchemaRead, SchemaWrite)]
pub(super) struct Chunk {
    pub schema: u32,
    pub start: u64,
    pub end: u64,
    pub parent: [u8; 32],
    pub blocks: Vec<Block>,
    pub accounts: BTreeMap<[u8; 20], Vec<(u64, Option<Info>)>>,
    #[allow(clippy::type_complexity)] // Keep the versioned wire representation explicit.
    pub storage: BTreeMap<([u8; 20], [u8; 32]), Vec<(u64, [u8; 32])>>,
    pub code: BTreeMap<[u8; 32], Vec<u8>>,
    pub hashes: BTreeMap<u64, [u8; 32]>,
}

/// Canonical block and historical pre-change expectations, not replay-derived outputs.
#[derive(Debug, SchemaRead, SchemaWrite)]
pub(super) struct Block {
    pub hash: [u8; 32],
    pub rlp: Vec<u8>,
    pub senders: Vec<[u8; 20]>,
    pub accounts: Vec<([u8; 20], Option<Info>)>,
    pub storage: Vec<([u8; 20], [u8; 32], [u8; 32])>,
}

#[derive(Clone, Debug, SchemaRead, SchemaWrite)]
pub(super) struct Info {
    nonce: u64,
    balance: [u8; 32],
    code_hash: Option<[u8; 32]>,
}

impl Info {
    pub fn capture(account: &Account) -> eyre::Result<Self> {
        let info = Self {
            nonce: account.nonce,
            balance: account.balance.to_be_bytes(),
            code_hash: account.bytecode_hash.map(|h| h.0),
        };
        eyre::ensure!(info.account() == *account, "unsupported account extension");
        Ok(info)
    }

    fn account(&self) -> Account {
        Account {
            nonce: self.nonce,
            balance: U256::from_be_bytes(self.balance),
            bytecode_hash: self.code_hash.map(B256::from),
        }
    }
}

/// Discovery records requests rather than cache contents: cache entries may be destroyed.
#[derive(Debug, Default)]
pub(super) struct Keys {
    pub accounts: BTreeSet<Address>,
    pub storage: BTreeSet<(Address, U256)>,
    pub code: BTreeSet<B256>,
    pub hashes: BTreeSet<u64>,
}

/// Placed beneath the configured executor's State, including native protocol storage.
#[derive(Debug)]
pub(super) struct Recording<D> {
    pub inner: D,
    pub keys: Arc<Mutex<Keys>>,
}

impl<D: Database> reth_revm::Database for Recording<D> {
    type Error = D::Error;
    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.keys.lock().accounts.insert(address);
        self.inner.basic(address)
    }
    fn storage(&mut self, address: Address, key: U256) -> Result<U256, Self::Error> {
        let mut keys = self.keys.lock();
        keys.accounts.insert(address);
        keys.storage.insert((address, key));
        drop(keys);
        self.inner.storage(address, key)
    }
    fn code_by_hash(&mut self, hash: B256) -> Result<Bytecode, Self::Error> {
        self.keys.lock().code.insert(hash);
        self.inner.code_by_hash(hash)
    }
    fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
        self.keys.lock().hashes.insert(number);
        self.inner.block_hash(number)
    }
}

/// A canonical state view at any reset boundary covered by a chunk.
#[derive(Clone, Debug)]
pub(super) struct View {
    pub chunk: Arc<Chunk>,
    pub at: u64,
}

#[derive(Debug)]
pub(super) struct Missing(pub String);
impl fmt::Display for Missing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for Missing {}
impl reth_revm::database_interface::DBErrorMarker for Missing {}

fn version<T>(versions: &[(u64, T)], at: u64) -> Result<&T, Missing> {
    versions
        .get(versions.partition_point(|(n, _)| *n <= at).wrapping_sub(1))
        .map(|(_, value)| value)
        .ok_or_else(|| Missing(format!("missing state version at {at}")))
}

impl reth_revm::Database for View {
    type Error = Missing;
    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Missing> {
        let values = self
            .chunk
            .accounts
            .get(&address.into_array())
            .ok_or_else(|| Missing(format!("uncaptured account {address}")))?;
        Ok(version(values, self.at)?.as_ref().map(|a| a.account().into()))
    }
    fn storage(&mut self, address: Address, key: U256) -> Result<U256, Missing> {
        let values = self
            .chunk
            .storage
            .get(&(address.into_array(), key.to_be_bytes()))
            .ok_or_else(|| Missing(format!("uncaptured native/storage {address} {key}")))?;
        Ok(U256::from_be_bytes(*version(values, self.at)?))
    }
    fn code_by_hash(&mut self, hash: B256) -> Result<Bytecode, Missing> {
        let code = self
            .chunk
            .code
            .get(&hash.0)
            .ok_or_else(|| Missing(format!("uncaptured code {hash}")))?;
        Bytecode::new_raw_checked(code.clone().into()).map_err(|err| Missing(err.to_string()))
    }
    fn block_hash(&mut self, number: u64) -> Result<B256, Missing> {
        self.chunk
            .hashes
            .get(&number)
            .copied()
            .map(B256::from)
            .ok_or_else(|| Missing(format!("uncaptured block hash {number}")))
    }
}

impl Chunk {
    /// Discover cold reads at every block, then bind their values to independent
    /// canonical history. A new contract's never-written zero slots are absent
    /// from changesets and can otherwise disappear behind the warm State cache.
    pub fn capture<E, D, P, C>(
        range: Range<u64>,
        config: &E,
        consensus: &C,
        expected: &P,
        mut db_at: impl FnMut(u64) -> eyre::Result<D>,
        mut block_at: impl FnMut(u64) -> eyre::Result<RecoveredBlock<BlockTy<E::Primitives>>>,
        cancellation: &CancellationToken,
    ) -> eyre::Result<Self>
    where
        E: ConfigureEvm,
        D: Database,
        D::Error: std::error::Error + Send + Sync + 'static,
        P: ChangeSetReader + StorageChangeSetReader,
        C: FullConsensus<E::Primitives> + ?Sized,
    {
        let Range { start, end } = range;
        eyre::ensure!(start > 0 && end > start, "empty/reversed corpus range");
        let keys = Arc::new(Mutex::new(Keys::default()));
        let mut blocks = Vec::new();
        let mut parent = None;
        let mut prior = None;
        for number in start..end {
            let block = block_at(number)?;
            eyre::ensure!(block.number() == number, "source block number mismatch");
            if let Some(prior) = prior {
                eyre::ensure!(block.parent_hash() == prior, "source canonical gap");
            }
            parent.get_or_insert(block.parent_hash().0);
            prior = Some(block.hash());
            super::execute::execute_chunk(
                config,
                consensus,
                expected,
                number..number + 1,
                false,
                cancellation,
                |at| Ok(Recording { inner: db_at(at)?, keys: keys.clone() }),
                |_| Ok(block.clone()),
                |_| Ok(Vec::new()),
                |_, _| {},
                |_, _| {},
            )?;
            let accounts = expected.account_block_changeset(number)?;
            let storage = expected.storage_changeset(number)?;
            {
                let mut keys = keys.lock();
                keys.accounts.extend(accounts.iter().map(|a| a.address));
                for (a, slot) in &storage {
                    keys.accounts.insert(a.address());
                    keys.storage.insert((a.address(), U256::from_be_bytes(slot.key.0)));
                }
            }
            blocks.push(Block {
                hash: block.hash().0,
                rlp: alloy_rlp::encode(block.clone_block()),
                senders: block.senders().iter().map(|a| a.into_array()).collect(),
                accounts: accounts
                    .iter()
                    .map(|a| {
                        Ok((
                            a.address.into_array(),
                            a.info.as_ref().map(Info::capture).transpose()?,
                        ))
                    })
                    .collect::<eyre::Result<_>>()?,
                storage: storage
                    .into_iter()
                    .map(|(a, s)| (a.address().into_array(), s.key.0, s.value.to_be_bytes()))
                    .collect(),
            });
        }
        let mut chunk = Self {
            schema: 1,
            start,
            end,
            parent: parent.ok_or_else(|| eyre::eyre!("empty chunk"))?,
            blocks,
            accounts: BTreeMap::new(),
            storage: BTreeMap::new(),
            code: BTreeMap::new(),
            hashes: BTreeMap::new(),
        };
        let keys = keys.lock();
        let mut initial = db_at(start - 1)?;
        for address in &keys.accounts {
            let info = initial.basic(*address)?;
            if let Some(info) = &info {
                keys_code_insert(&mut chunk.code, &mut initial, info.code_hash)?;
            }
            chunk.accounts.insert(
                address.into_array(),
                vec![(
                    start - 1,
                    info.as_ref().map(|a| Info::capture(&Account::from(a))).transpose()?,
                )],
            );
        }
        for (a, k) in &keys.storage {
            chunk.storage.insert(
                (a.into_array(), k.to_be_bytes()),
                vec![(start - 1, initial.storage(*a, *k)?.to_be_bytes())],
            );
        }
        for number in start..end {
            let mut canonical = db_at(number)?;
            let block = &chunk.blocks[(number - start) as usize];
            for (address, _) in &block.accounts {
                let info = canonical.basic(Address::from(*address))?;
                if let Some(info) = &info {
                    keys_code_insert(&mut chunk.code, &mut canonical, info.code_hash)?;
                }
                chunk.accounts.get_mut(address).expect("changeset discovery").push((
                    number,
                    info.as_ref().map(|a| Info::capture(&Account::from(a))).transpose()?,
                ));
            }
            for (a, k, _) in &block.storage {
                let value = canonical.storage(Address::from(*a), U256::from_be_bytes(*k))?;
                chunk
                    .storage
                    .get_mut(&(*a, *k))
                    .expect("changeset discovery")
                    .push((number, value.to_be_bytes()));
            }
        }
        for hash in &keys.code {
            keys_code_insert(&mut chunk.code, &mut initial, *hash)?;
        }
        for number in &keys.hashes {
            chunk.hashes.insert(*number, initial.block_hash(*number)?.0);
        }
        chunk.validate()?;
        Ok(chunk)
    }

    pub fn replay<E, C>(
        self: &Arc<Self>,
        config: &E,
        consensus: &C,
        cancellation: &CancellationToken,
    ) -> eyre::Result<(u64, u64)>
    where
        E: ConfigureEvm,
        C: FullConsensus<E::Primitives> + ?Sized,
    {
        self.validate()?;
        let mut prior = B256::from(self.parent);
        let mut blocks = Vec::with_capacity(self.blocks.len());
        let mut transactions = 0;
        for (index, input) in self.blocks.iter().enumerate() {
            let block: BlockTy<E::Primitives> = alloy_rlp::decode_exact(&input.rlp)?;
            eyre::ensure!(
                block.header().number() == self.start + index as u64 &&
                    block.header().parent_hash() == prior,
                "corpus block gap/order mismatch"
            );
            eyre::ensure!(
                block.body().transactions().len() == input.senders.len(),
                "corpus sender count mismatch"
            );
            let block = RecoveredBlock::new_unhashed(
                block,
                input.senders.iter().copied().map(Address::from).collect(),
            );
            eyre::ensure!(block.hash().0 == input.hash, "corpus canonical hash mismatch");
            prior = block.hash();
            transactions += input.senders.len() as u64;
            blocks.push(block);
        }
        let mut gas = 0;
        super::execute::execute_chunk(
            config,
            consensus,
            self.as_ref(),
            self.start..self.end,
            false,
            cancellation,
            |at| Ok(View { chunk: self.clone(), at }),
            |number| Ok(blocks[(number - self.start) as usize].clone()),
            |_| Ok(Vec::new()),
            |_, used| gas += used,
            |_, _| {},
        )?;
        Ok((transactions, gas))
    }

    fn block(&self, number: u64) -> ProviderResult<&Block> {
        number
            .checked_sub(self.start)
            .and_then(|n| self.blocks.get(n as usize))
            .ok_or_else(|| ProviderError::other(Missing(format!("uncaptured changeset {number}"))))
    }

    pub fn validate(&self) -> eyre::Result<()> {
        eyre::ensure!(
            self.schema == 1 && self.start > 0 && self.end > self.start,
            "unsupported corpus schema/range"
        );
        eyre::ensure!(
            self.blocks.len() as u64 == self.end - self.start,
            "missing/extra corpus blocks"
        );
        for versions in self.accounts.values() {
            self.validate_versions(versions)?;
        }
        for versions in self.storage.values() {
            self.validate_versions(versions)?;
        }
        for (hash, code) in &self.code {
            eyre::ensure!(
                alloy_primitives::keccak256(code).0 == *hash ||
                    (*hash == [0; 32] && code.is_empty()),
                "corpus code hash mismatch"
            );
        }
        Ok(())
    }

    fn validate_versions<T>(&self, versions: &[(u64, T)]) -> eyre::Result<()> {
        eyre::ensure!(
            versions.first().map(|v| v.0) == Some(self.start - 1),
            "missing initial state version"
        );
        eyre::ensure!(
            versions.windows(2).all(|v| v[0].0 < v[1].0) &&
                versions.last().is_some_and(|v| v.0 < self.end),
            "invalid state version ordering"
        );
        Ok(())
    }
}

fn keys_code_insert<D: Database>(
    code: &mut BTreeMap<[u8; 32], Vec<u8>>,
    db: &mut D,
    hash: B256,
) -> eyre::Result<()>
where
    D::Error: std::error::Error + Send + Sync + 'static,
{
    if let std::collections::btree_map::Entry::Vacant(entry) = code.entry(hash.0) {
        entry.insert(db.code_by_hash(hash)?.original_bytes().to_vec());
    }
    Ok(())
}

impl ChangeSetReader for Chunk {
    fn account_block_changeset(&self, n: u64) -> ProviderResult<Vec<AccountBeforeTx>> {
        Ok(self
            .block(n)?
            .accounts
            .iter()
            .map(|(a, info)| AccountBeforeTx {
                address: Address::from(*a),
                info: info.as_ref().map(Info::account),
            })
            .collect())
    }
    fn get_account_before_block(
        &self,
        n: u64,
        a: Address,
    ) -> ProviderResult<Option<AccountBeforeTx>> {
        Ok(self.account_block_changeset(n)?.into_iter().find(|v| v.address == a))
    }
    fn account_changesets_range(
        &self,
        range: impl RangeBounds<u64>,
    ) -> ProviderResult<Vec<(u64, AccountBeforeTx)>> {
        let mut result = Vec::new();
        for n in (self.start..self.end).filter(|n| range.contains(n)) {
            result.extend(self.account_block_changeset(n)?.into_iter().map(|a| (n, a)));
        }
        Ok(result)
    }
}

impl StorageChangeSetReader for Chunk {
    fn storage_changeset(&self, n: u64) -> ProviderResult<Vec<(BlockNumberAddress, StorageEntry)>> {
        Ok(self
            .block(n)?
            .storage
            .iter()
            .map(|(a, k, v)| {
                (
                    BlockNumberAddress::from((n, Address::from(*a))),
                    StorageEntry { key: B256::from(*k), value: U256::from_be_bytes(*v) },
                )
            })
            .collect())
    }
    fn get_storage_before_block(
        &self,
        n: u64,
        a: Address,
        key: B256,
    ) -> ProviderResult<Option<StorageEntry>> {
        Ok(self
            .storage_changeset(n)?
            .into_iter()
            .find(|(b, s)| b.address() == a && s.key == key)
            .map(|(_, s)| s))
    }
    fn storage_changesets_range(
        &self,
        range: impl RangeBounds<u64>,
    ) -> ProviderResult<Vec<(BlockNumberAddress, StorageEntry)>> {
        let mut result = Vec::new();
        for n in (self.start..self.end).filter(|n| range.contains(n)) {
            result.extend(self.storage_changeset(n)?);
        }
        Ok(result)
    }
}

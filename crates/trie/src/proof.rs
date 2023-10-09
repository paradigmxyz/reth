use crate::{
    account::EthAccount,
    hashed_cursor::{HashedAccountCursor, HashedCursorFactory},
    prefix_set::PrefixSet,
    trie_cursor::{AccountTrieCursor, TrieCursor},
    walker::TrieWalker,
    ProofError, StorageRoot,
};
use alloy_rlp::Encodable;
use reth_db::{cursor::DbCursorRO, tables, transaction::DbTx};
use reth_primitives::{
    keccak256,
    trie::{
        nodes::{rlp_hash, BranchNode, LeafNode, CHILD_INDEX_RANGE},
        BranchNodeCompact, HashBuilder, Nibbles,
    },
    Address, Bytes, B256,
};

/// A struct for generating merkle proofs.
///
/// Proof generator starts with acquiring the trie walker and restoring the root node in the trie.
/// The root node is restored from its immediate children which are stored in the database.
///
/// Upon encountering the child of the root node that matches the prefix of the requested account's
/// hashed key, the proof generator traverses the path down to the leaf node (excluded as we don't
/// store leaf nodes in the database). The proof generator stops traversing the path upon
/// encountering a branch node with no children matching the hashed key.
///
/// After traversing the branch node path, the proof generator attempts to restore the leaf node of
/// the target account by looking up the target account info.
/// If the leaf node exists, we encoded it and add it to the proof thus proving **inclusion**.
/// If the leaf node does not exist, we return the proof as is thus proving **exclusion**.
///
/// After traversing the path, the proof generator continues to restore the root node of the trie
/// until completion. The root node is then inserted at the start of the proof.
#[derive(Debug)]
pub struct Proof<'a, 'b, TX, H> {
    /// A reference to the database transaction.
    tx: &'a TX,
    /// The factory for hashed cursors.
    hashed_cursor_factory: &'b H,
}

impl<'a, TX> Proof<'a, 'a, TX, TX> {
    /// Create a new [Proof] instance.
    pub fn new(tx: &'a TX) -> Self {
        Self { tx, hashed_cursor_factory: tx }
    }
}

impl<'a, 'b, 'tx, TX, H> Proof<'a, 'b, TX, H>
where
    TX: DbTx<'tx>,
    H: HashedCursorFactory<'b>,
{
    /// Generate an account proof from intermediate nodes.
    pub fn account_proof(&self, address: Address) -> Result<Vec<Bytes>, ProofError> {
        let hashed_address = keccak256(address);
        let target_nibbles = Nibbles::unpack(hashed_address);

        let mut proof_restorer =
            ProofRestorer::new(self.tx)?.with_hashed_cursor_factory(self.hashed_cursor_factory)?;
        let mut trie_cursor =
            AccountTrieCursor::new(self.tx.cursor_read::<tables::AccountsTrie>()?);

        // Create the walker and immediately advance it from the root key.
        let mut walker = TrieWalker::new(&mut trie_cursor, PrefixSet::default());
        walker.advance()?;

        // Create a hash builder to rebuild the root node since it is not available in the database.
        let mut root_node_hash_builder = HashBuilder::default();

        let mut proofs: Vec<Bytes> = Vec::new();
        while let Some(key) = walker.key() {
            if target_nibbles.has_prefix(&key) {
                debug_assert!(proofs.is_empty(), "Prefix must match a single key");
                proofs = self.traverse_path(walker.cursor, &mut proof_restorer, hashed_address)?;
            }

            let value = walker.hash().unwrap();
            let is_in_db_trie = walker.children_are_in_trie();
            root_node_hash_builder.add_branch(key.clone(), value, is_in_db_trie);
            walker.advance()?;
        }

        // TODO: This is a hack to retrieve the root node from the hash builder.
        // We should find a better way.
        root_node_hash_builder.set_updates(true);
        let _ = root_node_hash_builder.root();
        let (_, mut updates) = root_node_hash_builder.split();
        let root_node = updates.remove(&Nibbles::default()).expect("root node is present");

        // Restore the root node RLP and prepend it to the proofs result
        let root_node_rlp = proof_restorer.restore_branch_node(&Nibbles::default(), root_node)?;
        proofs.insert(0, root_node_rlp);

        Ok(proofs)
    }

    fn traverse_path<T: DbCursorRO<'a, tables::AccountsTrie>>(
        &self,
        trie_cursor: &mut AccountTrieCursor<T>,
        proof_restorer: &mut ProofRestorer<'a, 'b, TX, H>,
        hashed_address: B256,
    ) -> Result<Vec<Bytes>, ProofError> {
        let mut intermediate_proofs = Vec::new();

        let target = Nibbles::unpack(hashed_address);
        let mut current_prefix = target.slice(0, 1);
        while let Some((_, node)) =
            trie_cursor.seek_exact(current_prefix.hex_data.to_vec().into())?
        {
            let branch_node_rlp = proof_restorer.restore_branch_node(&current_prefix, node)?;
            intermediate_proofs.push(branch_node_rlp);

            if current_prefix.len() < target.len() {
                current_prefix.extend([target.0[current_prefix.len()]]);
            }
        }

        if let Some(leaf_node_rlp) =
            proof_restorer.restore_target_leaf_node(hashed_address, current_prefix.len())?
        {
            intermediate_proofs.push(leaf_node_rlp);
        }

        Ok(intermediate_proofs)
    }
}

struct ProofRestorer<'a, 'b, TX, H>
where
    H: HashedCursorFactory<'b>,
{
    /// A reference to the database transaction.
    tx: &'a TX,
    /// The factory for hashed cursors.
    hashed_cursor_factory: &'b H,
    /// The hashed account cursor.
    hashed_account_cursor: H::AccountCursor,
    /// Pre-allocated buffer for account RLP encoding
    account_rlp_buf: Vec<u8>,
    /// Pre-allocated buffer for branch/leaf node RLP encoding
    node_rlp_buf: Vec<u8>,
}

impl<'a, 'tx, TX> ProofRestorer<'a, 'a, TX, TX>
where
    TX: DbTx<'tx>,
{
    fn new(tx: &'a TX) -> Result<Self, ProofError> {
        let hashed_account_cursor = tx.hashed_account_cursor()?;
        Ok(Self {
            tx,
            hashed_cursor_factory: tx,
            hashed_account_cursor,
            account_rlp_buf: Vec::with_capacity(128),
            node_rlp_buf: Vec::with_capacity(128),
        })
    }
}

impl<'a, 'b, 'tx, TX, H> ProofRestorer<'a, 'b, TX, H>
where
    TX: DbTx<'tx> + HashedCursorFactory<'a>,
    H: HashedCursorFactory<'b>,
{
    /// Set the hashed cursor factory.
    fn with_hashed_cursor_factory<'c, HF>(
        self,
        hashed_cursor_factory: &'c HF,
    ) -> Result<ProofRestorer<'a, 'c, TX, HF>, ProofError>
    where
        HF: HashedCursorFactory<'c>,
    {
        let hashed_account_cursor = hashed_cursor_factory.hashed_account_cursor()?;
        Ok(ProofRestorer {
            tx: self.tx,
            hashed_cursor_factory,
            hashed_account_cursor,
            account_rlp_buf: self.account_rlp_buf,
            node_rlp_buf: self.node_rlp_buf,
        })
    }

    fn restore_branch_node(
        &mut self,
        prefix: &Nibbles,
        node: BranchNodeCompact,
    ) -> Result<Bytes, ProofError> {
        let mut hash_idx = 0;
        let mut branch_node_stack = Vec::with_capacity(node.state_mask.count_ones() as usize);

        for child in CHILD_INDEX_RANGE.filter(|ch| node.state_mask.is_bit_set(*ch)) {
            if node.hash_mask.is_bit_set(child) {
                branch_node_stack.push(rlp_hash(node.hashes[hash_idx]));
                hash_idx += 1;
            } else {
                let child_key = prefix.join(&Nibbles::from_hex(Vec::from([child])));
                let mut child_key_to_seek = child_key.pack();
                child_key_to_seek.resize(32, 0);

                let leaf_node_rlp =
                    self.restore_leaf_node(B256::from_slice(&child_key_to_seek), child_key.len())?;
                branch_node_stack.push(leaf_node_rlp.to_vec());
            }
        }

        self.node_rlp_buf.clear();
        BranchNode::new(&branch_node_stack).rlp(node.state_mask, &mut self.node_rlp_buf);
        Ok(Bytes::copy_from_slice(self.node_rlp_buf.as_slice()))
    }

    /// Restore leaf node.
    /// The leaf nodes are always encoded as `RLP(node) or RLP(keccak(RLP(node)))`.
    fn restore_leaf_node(&mut self, seek_key: B256, slice_at: usize) -> Result<Bytes, ProofError> {
        let (hashed_address, account) = self
            .hashed_account_cursor
            .seek(seek_key)?
            .ok_or(ProofError::LeafAccountMissing(seek_key))?;

        // Restore account's storage root.
        let storage_root = StorageRoot::new_hashed(self.tx, hashed_address)
            .with_hashed_cursor_factory(self.hashed_cursor_factory)
            .root()?;

        self.account_rlp_buf.clear();
        EthAccount::from(account).with_storage_root(storage_root).encode(&mut self.account_rlp_buf);

        let leaf_node_key = Nibbles::unpack(hashed_address).slice_from(slice_at);
        let leaf_node = LeafNode::new(&leaf_node_key, &self.account_rlp_buf);

        self.node_rlp_buf.clear();
        Ok(Bytes::from(leaf_node.rlp(&mut self.node_rlp_buf)))
    }

    /// Restore target leaf node.
    /// The target node has to have an exactly matching key and is always encoded as `RLP(node)`.
    /// The target node might be missing from the trie.
    fn restore_target_leaf_node(
        &mut self,
        seek_key: B256,
        slice_at: usize,
    ) -> Result<Option<Bytes>, ProofError> {
        let (hashed_address, account) = match self.hashed_account_cursor.seek(seek_key)? {
            Some(entry) if entry.0 == seek_key => entry,
            _ => return Ok(None),
        };

        // Restore account's storage root.
        let storage_root = StorageRoot::new_hashed(self.tx, hashed_address)
            .with_hashed_cursor_factory(self.hashed_cursor_factory)
            .root()?;

        self.account_rlp_buf.clear();
        EthAccount::from(account).with_storage_root(storage_root).encode(&mut self.account_rlp_buf);

        let leaf_node_key = Nibbles::unpack(hashed_address).slice_from(slice_at);
        let leaf_node = LeafNode::new(&leaf_node_key, &self.account_rlp_buf);

        self.node_rlp_buf.clear();
        leaf_node.rlp(&mut self.node_rlp_buf);
        Ok(Some(Bytes::copy_from_slice(self.node_rlp_buf.as_slice())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StateRoot;
    use reth_db::{database::Database, test_utils::create_test_rw_db};
    use reth_interfaces::RethResult;
    use reth_primitives::{ChainSpec, StorageEntry, MAINNET};
    use reth_provider::{HashingWriter, ProviderFactory};
    use std::{str::FromStr, sync::Arc};

    fn insert_genesis<DB: Database>(db: DB, chain_spec: Arc<ChainSpec>) -> RethResult<()> {
        let provider_factory = ProviderFactory::new(db, chain_spec.clone());
        let mut provider = provider_factory.provider_rw()?;

        // Hash accounts and insert them into hashing table.
        let genesis = chain_spec.genesis();
        let alloc_accounts =
            genesis.alloc.clone().into_iter().map(|(addr, account)| (addr, Some(account.into())));
        provider.insert_account_for_hashing(alloc_accounts).unwrap();

        let alloc_storage = genesis.alloc.clone().into_iter().filter_map(|(addr, account)| {
            // Only return `Some` if there is storage.
            account.storage.map(|storage| {
                (
                    addr,
                    storage
                        .into_iter()
                        .map(|(key, value)| StorageEntry { key, value: value.into() }),
                )
            })
        });
        provider.insert_storage_for_hashing(alloc_storage)?;

        let (_, updates) = StateRoot::new(provider.tx_ref())
            .root_with_updates()
            .map_err(Into::<reth_db::DatabaseError>::into)?;
        updates.flush(provider.tx_mut())?;

        provider.commit()?;

        Ok(())
    }

    #[test]
    fn genesis_account_proof() {
        // Create test database and insert genesis accounts.
        let db = create_test_rw_db();
        insert_genesis(db.clone(), MAINNET.clone()).unwrap();

        // Address from mainnet genesis allocation.
        // keccak256 - `0xcf67b71c90b0d523dd5004cf206f325748da347685071b34812e21801f5270c4`
        let target = Address::from_str("0x000d836201318ec6899a67540690382780743280").unwrap();

        // `cast proof 0x000d836201318ec6899a67540690382780743280 --block 0`
        let expected_account_proof = [
            "0xf90211a090dcaf88c40c7bbc95a912cbdde67c175767b31173df9ee4b0d733bfdd511c43a0babe369f6b12092f49181ae04ca173fb68d1a5456f18d20fa32cba73954052bda0473ecf8a7e36a829e75039a3b055e51b8332cbf03324ab4af2066bbd6fbf0021a0bbda34753d7aa6c38e603f360244e8f59611921d9e1f128372fec0d586d4f9e0a04e44caecff45c9891f74f6a2156735886eedf6f1a733628ebc802ec79d844648a0a5f3f2f7542148c973977c8a1e154c4300fec92f755f7846f1b734d3ab1d90e7a0e823850f50bf72baae9d1733a36a444ab65d0a6faaba404f0583ce0ca4dad92da0f7a00cbe7d4b30b11faea3ae61b7f1f2b315b61d9f6bd68bfe587ad0eeceb721a07117ef9fc932f1a88e908eaead8565c19b5645dc9e5b1b6e841c5edbdfd71681a069eb2de283f32c11f859d7bcf93da23990d3e662935ed4d6b39ce3673ec84472a0203d26456312bbc4da5cd293b75b840fc5045e493d6f904d180823ec22bfed8ea09287b5c21f2254af4e64fca76acc5cd87399c7f1ede818db4326c98ce2dc2208a06fc2d754e304c48ce6a517753c62b1a9c1d5925b89707486d7fc08919e0a94eca07b1c54f15e299bd58bdfef9741538c7828b5d7d11a489f9c20d052b3471df475a051f9dd3739a927c89e357580a4c97b40234aa01ed3d5e0390dc982a7975880a0a089d613f26159af43616fd9455bb461f4869bfede26f2130835ed067a8b967bfb80",
            "0xf90211a0dae48f5b47930c28bb116fbd55e52cd47242c71bf55373b55eb2805ee2e4a929a00f1f37f337ec800e2e5974e2e7355f10f1a4832b39b846d916c3597a460e0676a0da8f627bb8fbeead17b318e0a8e4f528db310f591bb6ab2deda4a9f7ca902ab5a0971c662648d58295d0d0aa4b8055588da0037619951217c22052802549d94a2fa0ccc701efe4b3413fd6a61a6c9f40e955af774649a8d9fd212d046a5a39ddbb67a0d607cdb32e2bd635ee7f2f9e07bc94ddbd09b10ec0901b66628e15667aec570ba05b89203dc940e6fa70ec19ad4e01d01849d3a5baa0a8f9c0525256ed490b159fa0b84227d48df68aecc772939a59afa9e1a4ab578f7b698bdb1289e29b6044668ea0fd1c992070b94ace57e48cbf6511a16aa770c645f9f5efba87bbe59d0a042913a0e16a7ccea6748ae90de92f8aef3b3dc248a557b9ac4e296934313f24f7fced5fa042373cf4a00630d94de90d0a23b8f38ced6b0f7cb818b8925fee8f0c2a28a25aa05f89d2161c1741ff428864f7889866484cef622de5023a46e795dfdec336319fa07597a017664526c8c795ce1da27b8b72455c49657113e0455552dbc068c5ba31a0d5be9089012fda2c585a1b961e988ea5efcd3a06988e150a8682091f694b37c5a0f7b0352e38c315b2d9a14d51baea4ddee1770974c806e209355233c3c89dce6ea049bf6e8df0acafd0eff86defeeb305568e44d52d2235cf340ae15c6034e2b24180",
            "0xf901f1a0cf67e0f5d5f8d70e53a6278056a14ddca46846f5ef69c7bde6810d058d4a9eda80a06732ada65afd192197fe7ce57792a7f25d26978e64e954b7b84a1f7857ac279da05439f8d011683a6fc07efb90afca198fd7270c795c835c7c85d91402cda992eaa0449b93033b6152d289045fdb0bf3f44926f831566faa0e616b7be1abaad2cb2da031be6c3752bcd7afb99b1bb102baf200f8567c394d464315323a363697646616a0a40e3ed11d906749aa501279392ffde868bd35102db41364d9c601fd651f974aa0044bfa4fe8dd1a58e6c7144da79326e94d1331c0b00373f6ae7f3662f45534b7a098005e3e48db68cb1dc9b9f034ff74d2392028ddf718b0f2084133017da2c2e7a02a62bc40414ee95b02e202a9e89babbabd24bef0abc3fc6dcd3e9144ceb0b725a0239facd895bbf092830390a8676f34b35b29792ae561f196f86614e0448a5792a0a4080f88925daff6b4ce26d188428841bd65655d8e93509f2106020e76d41eefa04918987904be42a6894256ca60203283d1b89139cf21f09f5719c44b8cdbb8f7a06201fc3ef0827e594d953b5e3165520af4fceb719e11cc95fd8d3481519bfd8ca05d0e353d596bd725b09de49c01ede0f29023f0153d7b6d401556aeb525b2959ba0cd367d0679950e9c5f2aa4298fd4b081ade2ea429d71ff390c50f8520e16e30880",
            "0xf87180808080808080a0dbee8b33c73b86df839f309f7ac92eee19836e08b39302ffa33921b3c6a09f66a06068b283d51aeeee682b8fb5458354315d0b91737441ede5e137c18b4775174a8080808080a0fe7779c7d58c2fda43eba0a6644043c86ebb9ceb4836f89e30831f23eb059ece8080",
            "0xf8719f20b71c90b0d523dd5004cf206f325748da347685071b34812e21801f5270c4b84ff84d80890ad78ebc5ac6200000a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
        ].into_iter().map(Bytes::from_str).collect::<Result<Vec<_>, _>>().unwrap();

        let tx = db.tx().unwrap();
        let proof = Proof::new(&tx).account_proof(target).unwrap();
        pretty_assertions::assert_eq!(proof, expected_account_proof);
    }

    #[test]
    fn genesis_account_proof_nonexistent() {
        // Create test database and insert genesis accounts.
        let db = create_test_rw_db();
        insert_genesis(db.clone(), MAINNET.clone()).unwrap();

        // Address that does not exist in mainnet genesis allocation.
        // keccak256 - `0x18f415ffd7f66bb1924d90f0e82fb79ca8c6d8a3473cd9a95446a443b9db1761`
        let target = Address::from_str("0x000d836201318ec6899a67540690382780743281").unwrap();

        // `cast proof 0x000d836201318ec6899a67540690382780743281 --block 0`
        let expected_account_proof = [
            "0xf90211a090dcaf88c40c7bbc95a912cbdde67c175767b31173df9ee4b0d733bfdd511c43a0babe369f6b12092f49181ae04ca173fb68d1a5456f18d20fa32cba73954052bda0473ecf8a7e36a829e75039a3b055e51b8332cbf03324ab4af2066bbd6fbf0021a0bbda34753d7aa6c38e603f360244e8f59611921d9e1f128372fec0d586d4f9e0a04e44caecff45c9891f74f6a2156735886eedf6f1a733628ebc802ec79d844648a0a5f3f2f7542148c973977c8a1e154c4300fec92f755f7846f1b734d3ab1d90e7a0e823850f50bf72baae9d1733a36a444ab65d0a6faaba404f0583ce0ca4dad92da0f7a00cbe7d4b30b11faea3ae61b7f1f2b315b61d9f6bd68bfe587ad0eeceb721a07117ef9fc932f1a88e908eaead8565c19b5645dc9e5b1b6e841c5edbdfd71681a069eb2de283f32c11f859d7bcf93da23990d3e662935ed4d6b39ce3673ec84472a0203d26456312bbc4da5cd293b75b840fc5045e493d6f904d180823ec22bfed8ea09287b5c21f2254af4e64fca76acc5cd87399c7f1ede818db4326c98ce2dc2208a06fc2d754e304c48ce6a517753c62b1a9c1d5925b89707486d7fc08919e0a94eca07b1c54f15e299bd58bdfef9741538c7828b5d7d11a489f9c20d052b3471df475a051f9dd3739a927c89e357580a4c97b40234aa01ed3d5e0390dc982a7975880a0a089d613f26159af43616fd9455bb461f4869bfede26f2130835ed067a8b967bfb80",
            "0xf90211a0586b1ddec8db4824154209d355a1989b6c43aa69aba36e9d70c9faa53e7452baa0f86db47d628c73764d74b9ccaed73b8486d97a7731d57008fc9efaf417411860a0d9faed7b9ea107b5d98524246c977e782377f976e34f70717e8b1207f2f9b981a00218f59ccedf797c95e27c56405b9bf16845050fb43e773b66b26bc6992744f5a0dbf396f480c4e024156644adea7c331688d03742369e9d87ab8913bc439ff975a0aced524f39b22c62a5be512ddbca89f0b89b47c311065ccf423dee7013c7ea83a0c06b05f80b237b403adc019c0bc95b5de935021b14a75cbc18509eec60dfd83aa085339d45c4a52b7d523c301701f1ab339964e9c907440cff0a871c98dcf8811ea03ae9f6b8e227ec9be9461f0947b01696f78524c4519a6dee9fba14d209952cf9a0af17f551f9fa1ba4be41d0b342b160e2e8468d7e98a65a2dbf9d5fe5d6928024a0b850ac3bc03e9a309cc59ce5f1ab8db264870a7a22786081753d1db91897b8e6a09e796a4904bd78cb2655b5f346c94350e2d5f0dbf2bc00ac00871cd7ba46b241a0f6f0377427b900529caf32abf32ba1eb93f5f70153aa50b90bf55319a434c252a0725eaf27c8ee07e9b2511a6d6a0d71c649d855e8a9ed26e667903e2e94ae47cba0e4139fb48aa1a524d47f6e0df80314b88b52202d7e853da33c276aa8572283a8a05e9003d54a45935fdebae3513dc7cd16626dc05e1d903ae7f47f1a35aa6e234580",
            "0xf901d1a0b7c55b381eb205712a2f5d1b7d6309ac725da79ab159cb77dc2783af36e6596da0b3b48aa390e0f3718b486ccc32b01682f92819e652315c1629058cd4d9bb1545a0e3c0cc68af371009f14416c27e17f05f4f696566d2ba45362ce5711d4a01d0e4a0bad1e085e431b510508e2a9e3712633a414b3fe6fd358635ab206021254c1e10a0f8407fe8d5f557b9e012d52e688139bd932fec40d48630d7ff4204d27f8cc68da08c6ca46eff14ad4950e65469c394ca9d6b8690513b1c1a6f91523af00082474c80a0630c034178cb1290d4d906edf28688804d79d5e37a3122c909adab19ac7dc8c5a059f6d047c5d1cc75228c4517a537763cb410c38554f273e5448a53bc3c7166e7a0d842f53ce70c3aad1e616fa6485d3880d15c936fcc306ec14ae35236e5a60549a0218ee2ee673c69b4e1b953194b2568157a69085b86e4f01644fa06ab472c6cf9a016a35a660ea496df7c0da646378bfaa9562f401e42a5c2fe770b7bbe22433585a0dd0fbbe227a4d50868cdbb3107573910fd97131ea8d835bef81d91a2fc30b175a06aafa3d78cf179bf055bd5ec629be0ff8352ce0aec9125a4d75be3ee7eb71f10a01d6817ef9f64fcbb776ff6df0c83138dcd2001bd752727af3e60f4afc123d8d58080"
        ].into_iter().map(Bytes::from_str).collect::<Result<Vec<_>, _>>().unwrap();

        let tx = db.tx().unwrap();
        let proof = Proof::new(&tx).account_proof(target).unwrap();
        pretty_assertions::assert_eq!(proof, expected_account_proof);
    }
}
